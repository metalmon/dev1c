use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use crate::cfg::{cfg_with, Connection};
use crate::peer_out::Flow;
use crate::session::State;
use common::check::Check;
use core::prelude::v1::test;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Строка и список БЕЗ фактов зачисления — тем же вызовом, каким они
/// звались до задачи 7. Эти две обёртки нарочно перекрывают `row`/`list` из
/// `use super::*` (одиночное имя сильнее глоба): тесты ниже проверяют слова
/// транспорта, группировку и порядок — всё то, к чему заявки и коды не
/// относятся вовсе, — и дописывать им четвёртым аргументом пустой `Enroll`
/// значило бы повторить одно и то же шестнадцать раз. Тесты САМОГО
/// зачисления зовут `super::row` и `super::list` со своими фактами.
fn row(c: &Connection, current: bool, l: Option<&State>) -> Row {
    super::row(&crate::cfg::Cfg::default(), c, current, l, &Enroll::default())
}

fn list(cfg: &Cfg, link: &dyn Fn(&str) -> Option<State>) -> Vec<Entry> {
    super::list(cfg, link, &|_| Enroll::default())
}

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
fn link_lines_say_what_happened_without_naming_an_environment() {
    // Ни адреса, ни рабочего места: среда уехала (11.11), и строка журнала
    // говорит только о том, что включается и что выключено.
    assert_eq!(link_line(&State::Connecting).unwrap().text, "включаю доступ к 1С");
    assert_eq!(link_line(&State::Off).unwrap().text, "отключено");
    assert!(link_line(&State::Up { since: Instant::now() }).is_none());
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

/// Набранная команда `serve` обслуживает — галочка «подключаться при запуске» ей не указ.
///
/// Это не предпочтение, а приказ: человек набрал команду (закон 6 спеки — «человек решает»). А галочку ставят единственным местом —
/// меню значка, которого на машине без рабочего стола нет; «нет записи — выключено». Отдай ей власть над `serve`, и подкоманда там
/// печатала бы «порты … Ctrl+C — отключиться», держала замок и не поднимала ни сеанса, ни публикации, ни моста — МОЛЧА, потому что
/// `gate` пуст и строке взяться негде. Этот тест падает, если галочка снова начнёт править `serve`.
#[test]
fn a_typed_serve_serves_whatever_the_autostart_checkbox_says() {
    use crate::gui::Start;
    assert_eq!(serving_at_start(Start::Serve, None, false), Ok(true), "галочки нет, а команда набрана — обслуживаем");
    assert_eq!(serving_at_start(Start::Serve, None, true), Ok(true), "с галочкой — то же самое, она здесь ничего не решает");
    // Замок — запрет, и его слушаются оба способа. Но исход разный: окно живёт дальше с запертым переключателем и причиной в
    // журнале, а `serve` обслуживать не станет вовсе, и причина уедет наружу отказом — молчать здесь нельзя.
    assert_eq!(serving_at_start(Start::Serve, Some("в комплекте нет mcp-1c"), true), Err("в комплекте нет mcp-1c"));
    for hidden in [false, true] {
        let w = Start::Window { hidden };
        assert_eq!(serving_at_start(w, None, false), Ok(false), "{w:?}: окном правит галочка, и она снята");
        assert_eq!(serving_at_start(w, None, true), Ok(true), "{w:?}: галочка стоит — обслуживаем");
        assert_eq!(serving_at_start(w, Some("не задан пользователь базы"), true), Ok(false), "{w:?}: замок не обходится");
    }
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

/// Спека, «Связь»: «нет» своей строки не имеет вовсе. «Ждёт разрешения» —
/// слово протокола, которого в программе нет, и `row` его сейчас не отдаёт
/// вовсе (находка живого прогона 9.11 — см. докстроку `row`); «разрешено» —
/// докстрока `row` объясняет словами, а не тестом: `State` их не различает
/// вовсе, проверять нечего.
///
/// Соединение нарочно НЕ локальное (`mode = "tunnel"`): умолчание
/// `Connection::default()` — локальный режим (30.10), а у него своя пара
/// слов, проверенная отдельным тестом ниже.
#[test]
fn row_folds_eight_contract_states_into_five_words() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    assert_eq!(row(&c, false, Some(&State::Up { since: Instant::now() })).state, "подключено");
    assert_eq!(row(&c, false, Some(&State::Connecting)).state, "подключается");
    assert_eq!(row(&c, false, Some(&State::Retrying(String::new()))).state, "подключается снова");
    assert_eq!(row(&c, false, Some(&State::Failed(String::new()))).state, "разорвано");
    assert_eq!(row(&c, false, None).state, "не подключено");
}

/// Живой прогон 9.11: `State::Off` значит «обслуживание не поднято», а не
/// «кто-то ждёт решения человека» — слова «ждёт разрешения» принадлежат
/// протоколу (зачисление стороны, диалог разрешения), которого в программе
/// нет. Прежде оно читалось как «ждёт разрешения» у ЛЮБОЙ нелокальной
/// карточки, которая просто не успела подключиться — владелец увидел это
/// как «на разработчика нельзя переключить», когда карточка, которую он
/// только открыл, улетела первой строкой наверх списка.
#[test]
fn a_card_that_never_connected_does_not_say_it_waits_for_permission() {
    let c = Connection { mode: "tunnel".into(), role: "accept".into(), ..Default::default() };
    assert_eq!(row(&c, true, Some(&State::Off)).state, "не подключено");
}

/// Круг починки 1 задачи 2, Minor 2: первый прогон правки выше слил
/// `Off` и `Connecting` в одно слово, хотя `Connecting` — настоящее
/// движение («подключается»), которое человек видит прямо сейчас, а не
/// протокольный стенд-ин, как был `Off`. Три состояния — три разных слова.
#[test]
fn connecting_keeps_its_own_word_apart_from_off_and_retrying() {
    let c = Connection { mode: "tunnel".into(), role: "accept".into(), ..Default::default() };
    let off = row(&c, true, Some(&State::Off)).state;
    let connecting = row(&c, true, Some(&State::Connecting)).state;
    let retrying = row(&c, true, Some(&State::Retrying(String::new()))).state;
    assert_eq!(off, "не подключено");
    assert_eq!(connecting, "подключается");
    assert_eq!(retrying, "подключается снова");
    assert_ne!(off, connecting);
    assert_ne!(connecting, retrying);
    assert_ne!(off, retrying);
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

/// Задача 7 плана «Карточка публикации и удаление»: карточка с неизвестной
/// ролью в файле (правка руками, опечатка) — не «принимаю подключения» по
/// умолчанию замыкания `category`, а своя, честная группа. `role()` у такой
/// карточки отдаёт `Err`, и прежний код читал это как «не dial», то есть
/// молча клал её в первую группу — экран обещал бы человеку то, чего в
/// файле нет (находка ревью: `Connection::validate` эту карточку отказом не
/// пропустит, а значит сюда она попадает только из непрочитанного заново
/// списка — то же окно, что уже открыто, пока файл правят руками).
#[test]
fn a_card_with_an_unreadable_role_does_not_stand_under_the_accepting_caption() {
    // Четвёртая подпись `list` — та же строка, что видит человек. Названа
    // здесь один раз, чтобы правка слова не требовала трёх правок в тесте:
    // стережётся ГРУППА, а не формулировка.
    const UNKNOWN_CAPTION: &str = "Непонятно, как подключаются";
    let cfg = Cfg {
        connections: vec![
            Connection { name: "Ромашка".into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
            Connection { name: "Сервер".into(), mode: "tunnel".into(), role: "сервер".into(), ..Default::default() },
        ],
        ..Default::default()
    };
    let l = list(&cfg, &none);
    let mut seen_unknown_caption = false;
    let mut row_group: Option<&'static str> = None;
    for e in &l {
        match e {
            Entry::Caption(c) => {
                if *c == UNKNOWN_CAPTION {
                    seen_unknown_caption = true;
                }
            }
            Entry::Row(r) if r.name == "Сервер" => {
                row_group = Some(if seen_unknown_caption { UNKNOWN_CAPTION } else { "до своей подписи" })
            }
            Entry::Row(_) => {}
        }
    }
    assert_eq!(row_group, Some(UNKNOWN_CAPTION), "строка с нераспознанной ролью стоит не под своей подписью: {l:?}");
    let captions: Vec<&str> = l
        .iter()
        .filter_map(|e| match e {
            Entry::Caption(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(captions, vec!["Принимаю подключения", UNKNOWN_CAPTION], "«Сервер» не должен считаться принимающим");
}

/// Две карточки, по одной на accept и dial — ровно столько, чтобы подписи
/// были нужны (`show_captions`).
fn cfg_accept_and_dial() -> Cfg {
    Cfg {
        connections: vec![
            Connection { name: "Василёк".into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
            Connection { name: "Клевер".into(), mode: "tunnel".into(), role: "dial".into(), ..Default::default() },
        ],
        ..Default::default()
    }
}

/// Заглушка-поиск состояния: «Василёк» просто не успел подключиться
/// (`State::Off` — та же заглушка, что держит `gui::link_body` для текущего
/// соединения до первого события сеанса), «Клевер» молчит (`None`, то есть
/// «не подключено» и для него). До находки живого прогона 9.11
/// `Some(State::Off)` читался как «ждёт разрешения» и уносил «Василёк» выше
/// подписи его роли — ровно то, что видел владелец, впервые открыв окно
/// глазами.
fn accept_role_not_yet_connected(name: &str) -> Option<State> {
    (name == "Василёк").then_some(State::Off)
}

/// Живой прогон 9.11, вторая половина находки: карточка, которая просто не
/// успела подключиться, уезжала первой строкой наверх списка, а подпись её
/// роли оставалась висеть без единой строки под собой — список обещал
/// «Принимаю подключения» и не показывал под этими словами ничего. Правило
/// «ждущие — первой строкой» (докстрока `list`) не виновато: виновато было
/// ложное состояние, из-за которого оно срабатывало ВСЕГДА у нелокальной
/// карточки.
#[test]
fn two_cards_of_different_roles_stand_under_their_own_captions() {
    let l = list(&cfg_accept_and_dial(), &accept_role_not_yet_connected);
    assert!(matches!(l[0], Entry::Caption(_)), "строка всплыла над подписями, хотя это просто неподключённая карточка: {l:?}");
    for (i, e) in l.iter().enumerate() {
        if let Entry::Caption(c) = e {
            assert!(matches!(l.get(i + 1), Some(Entry::Row(_))), "подпись «{c}» висит без единой строки под собой: {l:?}");
        }
    }
}

/// Находка 31 осмотра 10.11: правило «ждущий — первой строкой» ПО-ПРЕЖНЕМУ
/// оставляло подпись группы без единой строки под ней — только теперь по
/// настоящей причине, а не из-за ложного состояния, вылеченного 9.11:
/// карточка и правда ждёт ответа (код выдан), и в своей группе она одна.
///
/// Правило наверху не тронуто: ждущая строка стоит выше всех подписей.
/// Переменилось одно — подпись отвечает за то, что под НЕЙ, и пустую группу
/// больше не подписывает.
#[test]
fn a_group_whose_only_row_went_to_the_top_shows_no_caption() {
    let issued = |name: &str| {
        if name == "Василёк" {
            Enroll { code_issued: true, ..Enroll::default() }
        } else {
            Enroll::default()
        }
    };
    let l = super::list(&cfg_accept_and_dial(), &none, &issued);
    assert!(matches!(l[0], Entry::Row(_)), "ждущая строка стоит выше всех подписей: {l:?}");
    let captions: Vec<&str> = l
        .iter()
        .filter_map(|e| match e {
            Entry::Caption(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(captions, vec!["Подключаюсь сам"], "группа, из которой строка уехала наверх, подписи не получает: {l:?}");
    for (i, e) in l.iter().enumerate() {
        if let Entry::Caption(c) = e {
            assert!(matches!(l.get(i + 1), Some(Entry::Row(_))), "подпись «{c}» висит без единой строки под собой: {l:?}");
        }
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

// Задача 7 плана «Канал и зачисление»: состояния контракта в окне.

/// Висящая заявка — та, что человек закрыл не решив (`Decision::NoAnswer`):
/// она НЕ разрешена и НЕ отклонена, и слово о ней обязано вернуться на
/// экран. Вместе со словом оживает и правило `list` «ждущий разрешения —
/// первой строкой», которое ждало именно этого случая.
///
/// Падал так: `row` слова не отдавала, `waiting` поднимать было нечего —
/// строка стояла под своей подписью, как любая другая.
#[test]
fn a_hanging_request_says_it_waits_and_stands_above_every_caption() {
    let cfg = cfg_accept_dial_local();
    let waits = |name: &str| Enroll { waiting: name == "Клевер", ..Default::default() };
    let l = super::list(&cfg, &none, &waits);
    let Some(Entry::Row(first)) = l.first() else {
        panic!("первой обязана стоять строка, а не подпись: {l:?}");
    };
    assert_eq!(first.name, "Клевер");
    assert_eq!(first.state, "ждёт разрешения");
    // Подпись группы, из которой строка уехала наверх, НЕ показывается
    // (находка 31 осмотра 10.11): подпись отвечает за то, что под ней, а под
    // ней не осталось ничего. Прежде тест требовал обратного — «её решает
    // полный список, а не то, что осталось под ней видимым», — и это
    // требование владелец отменил, увидев заголовок пустой группы на экране.
    assert!(!l.iter().any(|e| matches!(e, Entry::Caption("Подключаюсь сам"))), "подпись опустевшей группы не показывается: {l:?}");
}

/// Выданный код — то же ожидание, что и пришедшая заявка: человек ждёт,
/// чтобы с той стороны подключились. Своего слова он не получает.
#[test]
fn an_issued_code_reads_as_waiting_for_permission_too() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    let issued = Enroll { code_issued: true, ..Default::default() };
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &issued).state, "ждёт разрешения");
    // Код выдан, а связь уже поднялась — слово говорит про связь: ожидание
    // кончилось, и напоминать о нём незачем.
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, Some(&State::Up { since: Instant::now() }), &issued).state, "подключено");
}

/// Карточка с ВЫДАННЫМ кодом стоит в списке первой строкой — так же, как
/// карточка с висящей заявкой: ждут здесь одного и того же, и возвращать
/// человека к ожиданию должно самое заметное место.
///
/// Теста на МЕСТО этой строки не было вовсе, а решало его показанное слово:
/// `list` сравнивала готовую строку с литералом «ждёт разрешения». Правка
/// формулировки молча выключила бы правило — поэтому здесь не проверяется ни
/// одного слова, только место (`waits_for_answer` — решение по состоянию).
/// Красноту доказывает подмена слова в `contract_word` на закоммиченном
/// коде: прежняя `list` отправляла строку под подпись её группы, этот тест
/// падал, а соседний, что проверяет слово, оставался зелёным.
#[test]
fn an_issued_code_puts_the_card_first_whatever_the_word_says() {
    let cfg = cfg_accept_dial_local();
    // Код выдан НЕ первой карточке списка: иначе тест прошёл бы и без
    // правила — она и так стоит первой.
    let issued = |name: &str| Enroll { code_issued: name == "Клевер", ..Default::default() };
    let l = super::list(&cfg, &none, &issued);
    let Some(Entry::Row(first)) = l.first() else {
        panic!("карточка с выданным кодом обязана стоять первой строкой, выше всех подписей: {l:?}");
    };
    assert_eq!(first.name, "Клевер");
    // Подпись опустевшей группы не показывается — то же правило, что у
    // висящей заявки (находка 31).
    assert!(!l.iter().any(|e| matches!(e, Entry::Caption("Подключаюсь сам"))), "подпись опустевшей группы не показывается: {l:?}");

    // А локальная карточка наверх не всплывает ни при каком `Enroll`:
    // зачисления у неё нет и быть не может, и ждать ей нечего.
    let everyone = |_: &str| Enroll { code_issued: true, waiting: true, ..Default::default() };
    let l = super::list(&cfg, &none, &everyone);
    let names: Vec<&str> = l
        .iter()
        .take_while(|e| matches!(e, Entry::Row(_)))
        .map(|e| match e {
            Entry::Row(r) => r.name.as_str(),
            Entry::Caption(c) => c,
        })
        .collect();
    assert!(!names.contains(&"Эта машина"), "локальная карточка ожиданием не бывает: {l:?}");
}

/// Отказ — исход, а не ошибка (спека зачисления, шаг пятый): он держится
/// словом на строке, а не пропадает молча. Новая заявка его перебивает —
/// иначе человек, которого позвали второй раз, читал бы прошлый свой отказ
/// вместо приглашения ответить.
#[test]
fn a_refusal_is_an_outcome_and_a_new_request_overrides_it() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    let declined = Enroll { declined: true, ..Default::default() };
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &declined).state, "отклонено");
    let again = Enroll { declined: true, waiting: true, ..Default::default() };
    assert_eq!(
        super::row(&crate::cfg::Cfg::default(), &c, false, None, &again).state,
        "ждёт разрешения",
        "новая заявка важнее прошлого исхода"
    );
}

/// Зачисленная, но сейчас не подключённая сторона — это «не подключено», а
/// не «разрешено»: обещать движение там, где его нет, слово не вправе.
/// Число зачисленных живёт в карточке, а не в строке списка. Проверка, что
/// зачисление само по себе не состояние контракта, — за `contract_of`, и
/// живёт она рядом с ним (`contract::tests::being_enrolled_is_not_a_contract_state`).
#[test]
fn being_enrolled_is_not_a_word_on_the_row() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    let enrolled = Enroll { enrolled: 3, ..Default::default() };
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &enrolled).state, "не подключено");
}

/// Жизнь карточки — это её управляющий поток, а не транспорт рабочего места.
///
/// Падал так: пятого признака у `Enroll` не было вовсе, и слово состояния
/// читало `ui.link` — ОДИН сеанс SSH на машину. Четыре карточки с четырьмя
/// разными судьбами (одна звонит, вторая держит поток, третья оборвалась и
/// возвращается, четвёртая не заведена) показывали на экране одно и то же
/// слово, то есть врали про три из четырёх.
#[test]
fn each_card_shows_the_life_of_its_own_stream() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    let with = |f: Flow| Enroll { flow: Some(f), ..Default::default() };

    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &with(Flow::Calling)).state, "подключается");
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &with(Flow::Up)).state, "подключено");
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &with(Flow::Broken)).state, "разорвано");
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &with(Flow::Retrying)).state, "подключается снова");
}

/// Поток карточки важнее транспорта: там, где он есть, он и есть правда о
/// связи ЭТОЙ карточки — транспорт о ней не знает ничего.
///
/// И наоборот: где потока нет, слово по-прежнему берёт транспорт. Он живёт
/// параллельно и отвечает за сегодняшнюю работу — отобрать у него слово
/// значило бы сказать «не подключено» там, где сеанс поднят.
#[test]
fn the_cards_own_stream_outranks_the_machines_transport() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    let up = State::Up { since: Instant::now() };

    let broken_stream = Enroll { flow: Some(Flow::Broken), ..Default::default() };
    assert_eq!(
        super::row(&crate::cfg::Cfg::default(), &c, false, Some(&up), &broken_stream).state,
        "разорвано",
        "поток карточки важнее поднятого транспорта"
    );

    let no_stream = Enroll::default();
    assert_eq!(
        super::row(&crate::cfg::Cfg::default(), &c, false, Some(&up), &no_stream).state,
        "подключено",
        "потока нет — слово берёт транспорт"
    );
}

/// Висящая заявка перебивает даже живой поток: от неё ждут ДЕЙСТВИЯ
/// человека, и порядок разбора этого не меняет (вторая сторона стучится к
/// карточке, по которой первая уже подключена).
#[test]
fn a_waiting_request_still_wins_over_a_live_stream() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    let both = Enroll { waiting: true, flow: Some(Flow::Up), ..Default::default() };
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &both).state, "ждёт разрешения");
}

/// Локальной карточке зачислять некого — её пара слов не слушает `Enroll`
/// вовсе. Без этой защиты застрявший в файле выданный код сказал бы про
/// соединение, у которого оба конца здесь, «ждёт разрешения».
#[test]
fn a_local_card_keeps_its_own_two_words_whatever_the_book_says() {
    let c = Connection { mode: "local".into(), ..Default::default() };
    let noisy = Enroll { waiting: true, declined: true, code_issued: true, enrolled: 2, flow: Some(Flow::Up) };
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, None, &noisy).state, "выключено");
    assert_eq!(super::row(&crate::cfg::Cfg::default(), &c, false, Some(&State::Up { since: Instant::now() }), &noisy).state, "включено");
}

/// **Окно рассказывает управляющей двери о связи КАЖДОЙ карточки, и теми же словами, что строка списка** (задача 9: прежде оно клало
/// `cards: None` — «о связи пока не сообщаю», потому что класть было нечего).
///
/// Три карточки и три разных случая в одном снимке: текущая держит транспорт и получает его слово; соседняя туннельная о связи
/// молчит (своего потока у неё нет, транспорт — не её), и строки у неё нет вовсе; локальная не о связи вообще — у неё своя пара слов
/// на экране («включено»/«выключено»), и пусти её в снимок, `status` назвал бы это «подключено».
///
/// Слово сверяется с общим словарём, а не с литералом: сверка с «подключено» буквой пережила бы подмену словаря, ради единственности
/// которого он в бек и переезжал.
#[test]
fn the_window_tells_the_door_the_state_of_every_card() {
    let mut ui = Ui::for_test();
    ui.cfg = cfg_with(&["Ромашка", "Клевер", "Своя"]);
    for c in &mut ui.cfg.connections {
        c.mode = "tunnel".into();
    }
    ui.cfg.connections[2].mode = "local".into();
    ui.link = State::Up { since: Instant::now() };

    let v = door_view(&ui);
    let cards = v.cards.expect("окно о связи СООБЩАЕТ, а не молчит");
    assert_eq!(cards.len(), 1, "строку получает только та карточка, которой есть что сказать о связи: {cards:?}");
    assert_eq!(cards[0].name, "Ромашка", "транспорт принадлежит текущей карточке и только ей");
    assert_eq!(cards[0].word, contract_word(Contract::Up).unwrap(), "слово — из общего словаря, а не своё");

    // Связи нет ни у кого — и это СКАЗАНО пустым списком, а не умолчано `None`: разница названа типом (`door::StatusView::cards`).
    ui.link = State::Off;
    assert_eq!(door_view(&ui).cards, Some(vec![]), "связей нет — это ответ, а не незнание");
}

/// **Слово и цвет назначаются вместе, и сторож этого — здесь** (Important 4 ревью всей работы).
///
/// Чем куплен. Словарь состояний уехал в бек задачей 10, цвет точки по закону 8 остался в окне — и сопоставлялся со словом
/// СТРОКОВЫМ СРАВНЕНИЕМ. Правка формулировки в беке («разорвано» → «связь разорвана») молча перекрасила бы красную точку в жёлтую:
/// `assert` на `dot` в этом файле не было ни одного. Свёртка теперь читает ВАРИАНТ (`dot_of`), и забытый случай просто не
/// компилируется, — но пара «слово ↔ цвет» всё равно под присмотром, и присматривает за ней этот тест.
///
/// Ожидания записаны ЛИТЕРАЛАМИ нарочно: подставь сюда `contract_word`, и тест сверял бы строку с той самой функцией, из которой
/// она взята, — то есть не сверял бы ничего (правило ветки, круг правок 2 задачи 3). Поэтому правка слова в беке красит тест
/// красным и заставляет автора сказать вслух, остаётся ли цвет прежним.
///
/// Проба сделана: смени здесь любой цвет на соседний либо любое слово на знак — тест краснеет.
#[test]
fn a_word_and_a_colour_are_set_together() {
    use crate::gui::facts::Dot;
    let expected = |k: Contract| match k {
        Contract::No => (None, Dot::Off),
        Contract::CodeIssued => (Some("ждёт разрешения"), Dot::Warn),
        Contract::Waiting => (Some("ждёт разрешения"), Dot::Warn),
        Contract::Allowed => (Some("подключается"), Dot::Warn),
        Contract::Up => (Some("подключено"), Dot::Up),
        Contract::Broken => (Some("разорвано"), Dot::Down),
        Contract::Retrying => (Some("подключается снова"), Dot::Warn),
        Contract::Declined => (Some("отклонено"), Dot::Warn),
    };
    for k in CONTRACT_STATES {
        let (word, dot) = expected(k);
        assert_eq!(contract_word(k), word, "{k:?}: слово в беке переписали — проверьте цвет рядом и поправьте ожидание");
        assert_eq!(dot_of(Some(k)), dot, "{k:?}: цвет точки");
    }
    // Состояния контракта нет вовсе — обслуживание не поднято. Слово ставит `row`, цвет — та же серая точка, что у `Contract::No`.
    assert_eq!(dot_of(None), Dot::Off);
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    assert_eq!(row(&c, false, None).dot, Dot::Off, "«не подключено» — серая точка");
    assert_eq!(row(&c, false, Some(&State::Up { since: Instant::now() })).dot, Dot::Up, "«подключено» — зелёная");
    assert_eq!(row(&c, false, Some(&State::Failed(String::new()))).dot, Dot::Down, "«разорвано» — красная");
}
