use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use core::prelude::v1::test;
use crate::session::State;
use crate::verify;
use common::check::Check;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Вердикт из набора — задача 5. `Outcome::Bad` важнее `Outcome::Busy`
/// независимо от того, какой факт в наборе раньше: поломку человек
/// должен увидеть и при занятой базе, а не только свободной.
#[test]
fn the_verdict_reads_the_set_in_one_order() {
    let good = Check::pass("база", "есть");
    let busy = Check::busy("в базе открыт сеанс", "1 открыт сеанс 1С", "закройте Предприятие");
    let bad = Check::fail("проект", "не разложен", "1c-connect install");
    assert!(matches!(verdict_of(false, &[good.clone()]), Verdict::Off), "выключено — ничего не считаем");
    assert!(matches!(verdict_of(true, &[]), Verdict::Checking));
    assert!(matches!(verdict_of(true, &[good.clone()]), Verdict::Ready));
    assert!(matches!(verdict_of(true, &[busy.clone(), good]), Verdict::Busy(_)));
    // Поломка важнее занятости: её человек должен увидеть и при открытой базе.
    assert!(matches!(verdict_of(true, &[busy, bad]), Verdict::NotReady(_)));
}

/// Пошёл новый расчёт — набор на экране остаётся прежним, «Проверяю…» не
/// возвращается (задача 5): такт гоняет этот путь каждые секунды, и
/// сброс на каждый старт превратил бы карточку в мигающую.
#[test]
fn a_recalculation_keeps_the_previous_verdict_on_screen() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
    assert!(matches!(ui.verdict, Verdict::Ready));
    start_facts(&mut ui, Instant::now());
    assert!(matches!(ui.verdict, Verdict::Ready), "{:?}", ui.verdict);
}

/// Critical 1 ревью задач 5+6 — живая блокировка: «Проверить» зависало
/// навсегда. Пока держится `ui.busy`, такт идёт раз в секунду и каждый
/// раз просит повтор; прежний код в этом случае выбрасывал готовый
/// набор, не ставил отметку полного расчёта и не снимал `ui.busy` — то
/// есть карточка стояла на «Проверяю…» вечно, а `gather` ходил к базе
/// без паузы.
#[test]
fn a_set_that_arrives_while_a_repeat_is_pending_is_applied_and_ends_the_chain() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.busy = Some(Busy::Check);
    let gen = start_facts(&mut ui, Instant::now()).expect("расчёт пошёл");
    // Такт при идущем расчёте: второго потока нет, просьба о повторе есть.
    assert!(start_facts(&mut ui, Instant::now()).is_none());
    assert!(ui.facts_again);

    let again = apply_facts(&mut ui, gen, vec![Check::fail("проект", "не разложен", "install")]);
    assert!(again, "повтор обещан — он и будет, ровно один");
    assert!(matches!(ui.verdict, Verdict::NotReady(_)), "готовый набор выброшен: {:?}", ui.verdict);
    assert!(ui.facts_full_at.is_some(), "отметка полного расчёта не поставлена — такт просил бы повтор вечно");
    assert_eq!(ui.busy, None, "«Проверяю…» не снято — карточка стоит на нём навсегда");

    // Отметка свежая, и следующий такт на том же дешёвом наборе молчит —
    // череда расчётов кончилась сама, без отдельного тормоза.
    let since = Instant::now().saturating_duration_since(ui.facts_full_at.unwrap());
    assert!(!needs_full_pass(&ui.prev_cheap, &ui.prev_cheap, since, true));
    // Операцию ответ расчёта не трогает — её конец объявляет `Ev::Done`.
    ui.busy = Some(Busy::Project);
    let own = ui.facts_gen;
    apply_facts(&mut ui, own, Vec::new());
    assert_eq!(ui.busy, Some(Busy::Project));
}

/// Итог «Проверить» и есть вердикт (спека): полный набор в этот момент
/// уже посчитан, первый провал — причина в карточке. Без этой связки
/// живой случай заказчика 2026-10-20 повторяется дословно: точка
/// зелёная, а проверка тут же нашла два провала.
#[test]
fn a_finished_check_becomes_the_verdict() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    let gen = ui.facts_gen;
    let checks = vec![
        Check::pass("платформа 1С", "есть"),
        Check::note("комплект на носителе", "FAT32"),
        Check::fail("проект", "/home/dev3/project не разложен", "1c-connect install"),
        Check::fail("Gitea", "не отвечает", "dev1c-setup check"),
    ];
    apply_facts(&mut ui, gen, checks);
    match &ui.verdict {
        Verdict::NotReady(c) => assert_eq!(c.what, "проект", "первый провал, сведение пропущено"),
        other => panic!("{other:?}"),
    }
    // Проверка прошла — вердикт зелёный, тем же набором.
    apply_facts(&mut ui, gen, vec![Check::pass("платформа 1С", "есть"), Check::note("комплект на носителе", "FAT32")]);
    assert!(matches!(ui.verdict, Verdict::Ready), "{:?}", ui.verdict);
    // Пустой набор — «ещё не посчитано», а не прежний («всё хорошо»):
    // прежний вердикт мог быть и `Ready`, и точка осталась бы зелёной
    // при упавшей проверке.
    apply_facts(&mut ui, gen, Vec::new());
    assert!(matches!(ui.verdict, Verdict::Checking), "{:?}", ui.verdict);
}

/// Цепочка «Сохранить → публикация → операция» не должна поднимать три
/// пробы подряд и держать точку «занято» минуту: пока поток считает,
/// новое событие только помечает повтор.
#[test]
fn a_second_recompute_waits_for_the_first() {
    let mut ui = Ui::for_test();
    let gen = start_facts(&mut ui, Instant::now()).expect("первый расчёт идёт сам");
    assert!(ui.facts_pending);

    assert!(start_facts(&mut ui, Instant::now()).is_none(), "второго потока нет");
    assert!(ui.facts_again, "повтор помечен");
    assert_eq!(ui.facts_gen, gen, "поколение не трогаем — считает всё тот же поток");

    // Возврат потока: ровно один пересчёт, и ответ про старую почву не применён.
    assert!(apply_facts(&mut ui, gen, Vec::new()), "по возврату — пересчитать");
    assert!(!ui.facts_pending && !ui.facts_again);
    assert_eq!(start_facts(&mut ui, Instant::now()), Some(gen + 1));
}

/// Добавлено управляющим после ревью задач 3+4 (задача 6): «Проверить»
/// не плодит второй поход к базе, пока такт (или прежняя кнопка) уже
/// считает — защёлка одна на обоих. Кнопка не отказывает молча: она
/// помечает повтор (`facts_again`, та же отметка, что у такта) и
/// просит себе громкий показ (`check_requested`), который переживёт
/// чужой, отброшенный ответ.
#[test]
fn the_check_button_does_not_race_the_tick() {
    let mut ui = Ui::for_test();
    let gen = start_facts(&mut ui, Instant::now()).expect("такт начал расчёт первым");
    assert!(ui.facts_pending);

    assert!(request_check(&mut ui, Instant::now()).is_none(), "кнопка пошла в базу вторым потоком");
    assert!(ui.facts_again, "повтор помечен — тем же флагом, что у такта");
    assert!(ui.check_requested, "и громкий показ достанется ему");

    // Чужой (тактовый) ответ вернулся — он отброшен (`again`), но
    // отметка кнопки переживает чужой итог: громкий показ не потерян.
    assert!(apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]));
    assert!(ui.check_requested, "отметка кнопки не съедена чужим ответом");
}

/// Такт и поводы для пересчёта — задача 6. Изменившийся дешёвый факт
/// тянет дорогие независимо от срока; неизменившийся — молчит до срока,
/// который пореже при скрытом окне; а пропущенный надолго такт не
/// «протухает»: `since_full` просто больше любого порога.
#[test]
fn a_changed_cheap_fact_pulls_the_expensive_ones() {
    let free = vec![Check::pass("в базе открыт сеанс", "нет")];
    let busy = vec![Check::busy("в базе открыт сеанс", "1 открыт сеанс 1С", "закройте")];
    assert!(needs_full_pass(&busy, &free, Duration::ZERO, true), "закрыли базу — спросить заново");
    assert!(!needs_full_pass(&free, &free, Duration::from_secs(5), true), "ничего не менялось — молчим");
    assert!(needs_full_pass(&free, &free, SLOW_OPEN, true), "медленный такт при открытом окне");
    assert!(!needs_full_pass(&free, &free, SLOW_OPEN, false), "при закрытом окне такт реже");
    assert!(needs_full_pass(&free, &free, SLOW_HIDDEN, false));
    // Окно было скрыто сутки — набор не «протухает»: считаем сразу.
    assert!(needs_full_pass(&free, &free, Duration::from_secs(86_400), false));
}

/// Important 4 ревью задач 5+6: на потоке окна такт НЕ СЧИТАЕТ НИЧЕГО —
/// он только решает по отметкам времени, пора ли, и заводит поток. Прежний
/// такт звал на потоке окна `verify::cheap` и `configurator_count`:
/// 0,6–0,8 с и пять подпроцессов, ровно когда человек смотрит на карточку.
#[test]
fn the_window_thread_tick_only_raises_the_latch() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.sessions = Some(3);
    ui.prev_cheap = vec![Check::pass("в базе открыт сеанс", "нет")];

    // Защёлка СВОБОДНА, и такт правда доходит до конца (Important 3 ревью
    // круга 2): прежняя версия входила с `cheap_pending = true` и выходила
    // на собственном страже, так что все четыре `assert` были истинны
    // тривиально — верни `verify::cheap` на поток окна после стража, и
    // тест бы не заметил. Поток `spawn_cheap` до `Ui` не дотягивается, он
    // шлёт `Ev`, поэтому смотреть сразу после возврата можно без гонки.
    assert!(!ui.cheap_pending, "защёлка должна быть свободна — иначе тест идёт по раннему возврату");
    tick_facts(&mut ui, Instant::now(), true);
    assert!(ui.cheap_pending, "такт не завёл потока — тест проверяет не то");
    assert_eq!(ui.sessions, Some(3), "такт посчитал сеансы на потоке окна");
    assert_eq!(ui.prev_cheap.len(), 1, "такт посчитал дешёвый набор на потоке окна");
    assert!(ui.cheap_at.is_none(), "такт отметил дешёвый набор, которого не считал");
    assert!(!ui.facts_again && !ui.facts_pending, "такт сам полез за дорогими фактами");

    // Пока поток не ответил, второго такт не заводит.
    let before = ui.facts_gen;
    tick_facts(&mut ui, Instant::now(), true);
    assert_eq!(ui.facts_gen, before);

    // Выключено — не считается ничего, и поток не заводится (спека).
    ui.cheap_pending = false;
    ui.enabled.store(false, Ordering::Relaxed);
    tick_facts(&mut ui, Instant::now(), true);
    assert!(!ui.cheap_pending, "при выключенном обслуживании такт всё равно пошёл считать");
}

/// Important 8 ревью задач 5+6: проводка такта (Critical 1 и 3 живут
/// именно в ней) не была покрыта вовсе. Ответ дешёвого потока обязан
/// отпустить свою защёлку, не обогнать идущий расчёт, не позвать замок,
/// не терять изменение (Minor 11) и ничего не считать при выключенном
/// обслуживании.
#[test]
fn a_cheap_answer_asks_for_the_expensive_pass_without_overtaking_it() {
    let free = vec![Check::pass("в базе открыт сеанс", "нет")];
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.gate = None;
    // Полный расчёт уже идёт: эскалация обязана только попросить добавку.
    ui.facts_pending = true;
    ui.facts_since = Some(Instant::now());
    ui.cheap_pending = true;

    on_cheap(&mut ui, Some(0), Some(free.clone()), Instant::now(), true);
    assert!(!ui.cheap_pending, "защёлка дешёвого такта не отпущена — такт замолчал бы навсегда");
    assert!(ui.facts_again, "первый дешёвый набор не позвал дорогие факты");
    assert!(ui.cheap_at.is_some(), "отметка дешёвого набора не поставлена");
    assert!(ui.gate.is_none(), "такт позвал холодный замок (Critical 3)");
    assert_eq!(ui.sessions, Some(0));
    // Minor 11: расчёт получил только отметку повтора — прочитанное за ним
    // не запоминаем, иначе изменение пропадёт между тактом и ответом.
    assert!(ui.prev_cheap.is_empty(), "дешёвый набор запомнен расчётом, который его не читал");

    // Сменилось число открытых сеансов — повод переспросить базу сам по
    // себе, без всего дешёвого набора: ровно ради этого нужен секундный
    // такт («закрыл Предприятие — через секунду-две позеленело»).
    ui.facts_again = false;
    on_cheap(&mut ui, Some(1), None, Instant::now(), true);
    assert!(ui.facts_again, "открытое/закрытое «Предприятие» не позвало пересчёт");
    assert_eq!(ui.sessions, Some(1));

    // То же число, тот же дешёвый набор и свежая отметка полного расчёта —
    // повода нет, и прочитанное запоминается (сравнивать следующий такт
    // будет с ним).
    ui.facts_again = false;
    ui.facts_full_at = Some(Instant::now());
    ui.prev_cheap = free.clone();
    on_cheap(&mut ui, Some(1), Some(free.clone()), Instant::now(), true);
    assert!(!ui.facts_again, "такт зовёт дорогие факты на каждом тике");
    assert_eq!(ui.prev_cheap, free);

    // Выключили — ответ потока защёлку отпускает и больше ничего.
    ui.cheap_pending = true;
    ui.enabled.store(false, Ordering::Relaxed);
    on_cheap(&mut ui, Some(7), Some(Vec::new()), Instant::now(), true);
    assert!(!ui.cheap_pending);
    assert!(!ui.facts_again, "вердикт считается при выключенном обслуживании");
    assert_eq!(ui.sessions, Some(1), "при выключенном обслуживании такт что-то запомнил");
    assert_eq!(ui.prev_cheap, free);
}

/// Дефект (заказчик 2026-10-20): карточка не должна повторять подсказку,
/// уже вписанную в сам текст факта (TOOLS-Blocked несёт её внутри
/// `detail`, см. `ops_host::tools_state`), и никогда не говорит
/// «факт N провален».
#[test]
fn readiness_reason_says_what_and_what_to_do_without_echoing_the_hint() {
    let blocked = Check::fail(
        "инструменты комплекта",
        "в комплекте нет vrunner/, git/ или httpd/ — скопируйте полный комплект заново",
        "скопируйте полный комплект заново",
    );
    let text = readiness_reason(&blocked);
    assert_eq!(text.matches("скопируйте полный комплект заново").count(), 1, "{text}");
    assert!(!text.to_lowercase().contains("факт"), "{text}");

    let web = Check::fail("веб-сервер", "порт 1818 не отвечает", "запустите Apache/IIS, на котором опубликована база");
    let text = readiness_reason(&web);
    assert!(text.contains("порт 1818 не отвечает") && text.contains("запустите Apache/IIS"), "{text}");
}

/// Дефект (заказчик 2026-10-21): в строку итога вписали полный путь
/// отчёта, и она перестала помещаться в подвал — на снимке «Проблем: 2.
/// Подробности — отчё…». Итог называет только числа, а путь отчёта
/// уходит отдельной строкой журнала и подсказкой подвала.
#[test]
fn the_check_status_line_stays_short() {
    for failed in [1usize, 2, 99] {
        let l = check_status(failed);
        assert_eq!((l.kind, l.text.as_str()), (Kind::Fail, format!("Проблем: {failed}").as_str()));
        assert!(!l.text.contains("last-check"), "путь отчёта вернулся в строку подвала: {}", l.text);
        assert!(l.text.chars().count() <= 80, "строка итога не влезает в подвал: {}", l.text);
    }
    let ok = check_status(0);
    assert_eq!((ok.kind, ok.text.as_str()), (Kind::Ok, "Проверка пройдена"));
    assert!(ok.text.chars().count() <= 80, "{}", ok.text);
    // Путь не пропал — он уехал в строку журнала (она же подсказка подвала).
    let report = "/home/dev3/.local/share/dev1c/last-check.txt";
    assert_eq!(report_line(report), Line { kind: Kind::Say, text: format!("Отчёт — {report}") });
}

/// Решение 2 спеки: главная кнопка вкладки «Проект» выбирается по ФАКТУ
/// расширения из набора, а не по состоянию операций (`run::status`), и
/// имя факта берётся у того же `verify::EXTENSION`, что печатает отчёт —
/// своего, второго имени у окна нет.
#[test]
fn the_project_button_installs_until_the_extension_is_up_then_exports() {
    let ext = |c: Check| project_primary(&[Check::pass("база", "есть"), c]);
    // Набора ещё нет — кнопке сказать нечего: «Установить» было бы
    // утверждением о том, чего мы не знаем (Minor 4).
    assert_eq!(project_primary(&[]), None);
    assert_eq!(project_primary(&[Check::pass("база", "есть")]), None, "факта расширения нет — утверждать нечего");
    assert_eq!(ext(Check::fail(verify::EXTENSION, "расширения в базе нет", "install")), Some(Which::Install));
    // Расширение стоит — главная кнопка про обмен с git.
    assert_eq!(ext(Check::pass(verify::EXTENSION, "0.4.8")), Some(Which::Export));
    // Занято — не провал: советовать «Установить» человеку, который
    // просто работает в базе, нельзя (STATUS 26).
    assert_eq!(ext(Check::busy(verify::EXTENSION, "сеансов больше не осталось", "закройте «Предприятие»")), Some(Which::Export));
}

#[test]
fn checks_print_into_the_log_as_facts_with_a_hint_on_its_own_line() {
    let lines = check_lines(&[
        Check::pass("база", "C:\\Bases\\Buh"),
        Check::note("туннель", "не поднят"),
        Check::fail("расширение", "не установлено", "нажмите «Установить»"),
    ]);
    assert_eq!(lines.len(), 4);
    // Значки из текста ушли — их теперь даёт `Kind::mark` при отрисовке.
    assert_eq!(lines[0], Line { kind: Kind::Ok, text: "база: C:\\Bases\\Buh".into() });
    assert_eq!(lines[1], Line { kind: Kind::Say, text: "туннель: не поднят".into() });
    assert_eq!(lines[2], Line { kind: Kind::Fail, text: "расширение: не установлено".into() });
    assert_eq!(lines[3], Line { kind: Kind::Warn, text: "нажмите «Установить»".into() });
    // Пустая подсказка второй строки не добавляет.
    assert_eq!(check_lines(&[Check::fail("а", "б", "")]).len(), 1);
}

/// Critical 2 ревью задач 5+6 — «занято» горело ЗЕЛЁНЫМ. На стенде
/// человек с открытым «Предприятием» видел зелёную точку и «MCP
/// включены», хотя агенту в эту минуту откажут; владелец 25.10:
/// «не должно быть противоречий, когда MCP типа подключены, горит
/// зелёный, а в статусах что-то красным горит». Четыре поверхности —
/// карточка, подвал, подсказка значка и сам значок — обязаны сказать
/// «занято» словами факта, и ни одна из них не зелёная.
#[test]
fn a_busy_verdict_is_never_green_on_any_surface() {
    let now = Instant::now();
    // Пара, которую код правда рождает (Minor 9 ревью круга 2): отказ
    // базы по числу сеансов, словами и советом самой платформы. Прежде
    // тест лепил факт «в базе открыт сеанс» с советом «закройте
    // Предприятие» — такой пары `verify` уже не отдаёт, открытый сеанс
    // стал сведением.
    let cause = crate::onec_error::explain("Превышено количество сеансов").expect("ограничение узнаётся");
    let fact = Check::busy("расширение в базе", cause.what, cause.fix);
    let busy = Verdict::Busy(fact);
    let up = State::Up { since: now };

    // Карточка: точка не зелёная, заголовок — состояние, а не проза.
    let c = panel::card(None, &up, "local", &busy, None, now);
    assert_ne!(c.dot, Dot::Up, "«занято» показано зелёным: {:?}", c.title);
    assert_eq!((c.dot, c.title.as_str()), (Dot::Busy, "Занято"));
    assert_ne!(c.title, "MCP включены");

    // Подвал: причина — словами факта.
    let why = panel::reason(&up, &busy, None, Some("15:54")).expect("причина занятости названа");
    assert!(why.starts_with("Занято:"), "{why}");
    assert!(why.contains("расширение в базе") && why.contains(cause.what), "{why}");
    assert!(why.contains(cause.fix), "{why}");

    // Подсказка значка — та же причина, теми же словами.
    let tip = tray_tooltip(None, &up, &busy, None, now);
    assert!(tip.contains("занято") && tip.contains(cause.what), "{tip}");
    assert_ne!(tip, "1c-connect: подключён");

    // Значок: жёлтый «занято», не зелёный.
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.link = up;
    ui.gate = None;
    ui.verdict = busy;
    assert_eq!(tray_state_now(&ui, now), TrayState::Busy);

    // И журнал про занятость говорит — своими словами, не «не всё готово».
    let line = warn_reason(&ui.verdict).expect("журналу есть что сказать");
    assert!(line.starts_with("занято:"), "{line}");
}

/// Important 3 финального ревью ветки: «выключено» протекало двумя
/// дорогами, и обе кончались полным набором ПОСЛЕ выключения — поход в
/// базу, диск, до минуты, плюс `last-check.txt`, если расчёт громкий.
///
/// Сторож один и тот же (`start_full_pass`), и теперь обе дороги идут
/// через него.
#[test]
fn nothing_is_computed_after_the_switch_went_off() {
    // Дорога первая: обещанный повтор в `Ev::Ready`. Повтор просят, пока
    // обслуживание живо; выключают до того, как ответ пришёл.
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    let gen = start_facts(&mut ui, Instant::now()).expect("расчёт пошёл");
    assert!(start_facts(&mut ui, Instant::now()).is_none(), "второго потока нет — только просьба о повторе");
    assert!(ui.facts_again);
    ui.enabled.store(false, Ordering::Relaxed);
    let again = apply_facts(&mut ui, gen, vec![Check::fail("проект", "не разложен", "install")]);
    assert!(again, "повтор был обещан — решает о нём не этот тест");
    // Ровно то, что делает `Ev::Ready` на `again`.
    assert!(!start_full_pass(&mut ui), "набор считается после выключения");
    assert!(!ui.facts_pending, "защёлка взведена — значит поток ушёл в базу");

    // А при живом обслуживании тот же вызов расчёт заводит — значит
    // отказал именно сторож, а не что-то ещё.
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(start_full_pass(&mut ui), "сторож держит и включённое обслуживание");
    assert!(ui.facts_pending);
}

/// Дорога вторая того же Important 3: громкий показ. Нажатие «Проверить»
/// заводило расчёт и при выключенном обслуживании, `apply_facts` набор
/// потом ронял, и подвал говорил **«Проверка пройдена»** — на выключенном
/// коннекторе, при отчёте с провалами на диске. Сторож в `run_check`
/// (`serving`) эту дорогу закрывает, но сама ложь убита здесь, в чистом
/// решении: без набора говорить «пройдена» нечем.
#[test]
fn a_loud_show_never_says_passed_without_a_set() {
    // Набора нет — показывать нечего, и это не «пройдена».
    let line = loud_status(&[]);
    assert_eq!((line.kind, line.text.as_str()), (Kind::Warn, CHECK_OFF));
    assert_ne!(line.text, check_status(0).text, "пустой набор объявлен пройденной проверкой");
    // Набор есть — прежний итог, числами.
    assert_eq!(loud_status(&[Check::pass("база", "есть")]).text, "Проверка пройдена");
    assert_eq!(loud_status(&[Check::fail("проект", "не разложен", "install")]).text, "Проблем: 1");
    // Занято — не провал и в итоге тоже (решение владельца 25.10).
    assert_eq!(loud_status(&[Check::busy("в базе открыт сеанс", "сеансов не осталось", "закройте")]).text, "Проверка пройдена");
}

/// Minor 2 ревью задач 7+8: совет в журнале — у любого факта, которому
/// есть что добавить, а не только у провалившегося. У занятого факта
/// («закройте «Предприятие»») он терялся, и человек читал проблему без
/// выхода из неё.
#[test]
fn the_log_keeps_the_hint_of_a_busy_fact_too() {
    let lines = check_lines(&[
        Check::busy("в базе открыт сеанс", "сеансов больше не осталось", "закройте «Предприятие»"),
        // Совет уже вписан в сам `detail` — вторым разом не повторяем
        // (то же правило, по которому склеивает `fact_detail`).
        Check::fail("инструменты комплекта", "в комплекте нет vrunner — скопируйте заново", "скопируйте заново"),
    ]);
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!(texts[0], "в базе открыт сеанс: сеансов больше не осталось");
    assert_eq!((lines[1].kind, texts[1]), (Kind::Warn, "закройте «Предприятие»"));
    assert_eq!(texts[2], "инструменты комплекта: в комплекте нет vrunner — скопируйте заново");
}

/// Задача 8, решение 6 спеки: обычный расчёт молчит. Строка в журнал идёт
/// только на СМЕНЕ вердикта, и «Проверяю» новостью не бывает — это
/// отсутствие ответа, а не ответ.
#[test]
fn the_journal_speaks_only_when_the_verdict_changes() {
    let bad = Verdict::NotReady(Check::fail("проект", "не разложен", "1c-connect install"));
    let busy = Verdict::Busy(Check::busy("расширение в базе", "сеансов больше не осталось", "закройте «Предприятие»"));
    // То же самое — молчим, сколько бы расчётов ни пришло.
    assert!(verdict_line(&Verdict::Ready, &Verdict::Ready).is_none());
    assert!(verdict_line(&bad, &bad).is_none());
    // «Проверяю» и «выключено» — не новость ни после чего.
    assert!(verdict_line(&bad, &Verdict::Checking).is_none());
    assert!(verdict_line(&Verdict::Ready, &Verdict::Off).is_none());
    // Смена — строка, и словами самого факта.
    let line = verdict_line(&Verdict::Ready, &bad).expect("смена вердикта — строка");
    assert_eq!(line.kind, Kind::Warn);
    assert!(line.text.contains("проект") && line.text.contains("1c-connect install"), "{line:?}");
    // Жёлтое жёлтому не равно: «занято» говорит своими словами.
    let line = verdict_line(&bad, &busy).expect("с поломки на занятость — новость");
    assert!(line.text.starts_with("занято:"), "{line:?}");
    // Зелёное — тоже новость: починивший проблему человек читает об этом.
    let line = verdict_line(&bad, &Verdict::Ready).expect("починилось — новость");
    assert_eq!((line.kind, line.text.as_str()), (Kind::Ok, VERDICT_READY));
    // И сменившаяся ПРИЧИНА при том же состоянии — тоже смена.
    let other = Verdict::NotReady(Check::fail("Gitea", "не отвечает", "dev1c-setup check"));
    assert!(verdict_line(&bad, &other).is_some());

    // Проводка: два одинаковых набора подряд — одна строка, не две.
    // `mode = "local"` обязателен, иначе `serving` ложно (`Ui::for_test`
    // туннельный), набор не применится и тест остался бы зелёным на
    // сломанном коде.
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui), "иначе тест проверяет не то");
    let fact = || vec![Check::fail("проект", "не разложен", "1c-connect install")];
    let gen = ui.facts_gen;
    let before = ui.log.len();
    apply_facts(&mut ui, gen, fact());
    assert_eq!(ui.log.len(), before + 1, "о смене вердикта журнал не сказал");
    apply_facts(&mut ui, gen, fact());
    assert_eq!(ui.log.len(), before + 1, "тот же вердикт сказан дважды — журнал засыпан");
}

/// Задача 8: бейдж «Проблем: N» считает плохие факты ЖИВОГО набора.
/// Занято — не провал ни в окне, ни в отчёте (решение владельца 25.10):
/// человек, работающий в базе, получает 18 из 18 и строку «занято».
#[test]
fn the_badge_counts_only_bad_facts() {
    let facts = vec![
        Check::pass("база", "C:\\Bases\\Buh"),
        Check::note("комплект на носителе", "FAT32"),
        Check::busy("в базе открыт сеанс", "сеансов больше не осталось", "закройте «Предприятие»"),
        Check::fail("проект", "не разложен", "1c-connect install"),
    ];
    assert_eq!(problem_count(&facts), 1);
    // Пустой набор — не «ноль проблем доказано», но и показывать нечего:
    // бейджа нет, а состояние говорит карточка («Проверяю…»).
    assert_eq!(problem_count(&[]), 0);
    // Набор живой: применился новый — изменилось и число, без всякого
    // нажатия «Проверить».
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui), "иначе набор не применится и тест проверяет не то");
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, facts);
    assert_eq!(problem_count(&ui.facts), 1);
    apply_facts(&mut ui, gen, vec![Check::pass("проект", "в порядке")]);
    assert_eq!(problem_count(&ui.facts), 0, "бейдж держится за снимок, а не за живой набор");
}

/// Minor 10 ревью круга 2: пока набора нет, значок жёлтый (`Dot::Busy` из
/// `panel::card`), а подсказка отдавала «подключён» — зелёное слово под
/// жёлтым значком, та же ложь, что была в Critical 2, только в трее.
/// Спека: «значок в трее жёлтый, подсказка „проверяю“».
#[test]
fn the_tray_says_it_is_checking_while_there_is_no_set_yet() {
    let now = Instant::now();
    let up = State::Up { since: now };
    assert_eq!(tray_tooltip(None, &up, &Verdict::Checking, None, now), "1c-connect: проверяю");
    assert_eq!(panel::card(None, &up, "local", &Verdict::Checking, None, now).dot, Dot::Busy, "значок и подсказка обязаны говорить одно");
    // Посчитано и всё сошлось — прежняя подсказка про связь.
    assert_eq!(tray_tooltip(None, &up, &Verdict::Ready, None, now), "1c-connect: подключён");
    // Режима подсказка не спрашивает вовсе (Critical 1 финального ревью):
    // прежде здесь стояло «в туннельном режиме вердикт не показываем», и
    // это закрепляло дефект — значок желтел по вердикту, а подсказка
    // говорила «подключён». Один вердикт — одни слова на всех
    // поверхностях и в обоих режимах.
    let bad = Verdict::NotReady(Check::fail("проект", "не разложен", "1c-connect install"));
    let tip = tray_tooltip(None, &up, &bad, None, now);
    assert!(tip.contains("не всё готово") && tip.contains("проект"), "{tip}");
    assert_eq!(tip, format!("1c-connect: {}", warn_reason(&bad).unwrap()), "значок и подвал говорят разными словами");
}

/// Important 4 ревью круга 2: отчёт и строки в журнал без нажатия кнопки.
/// Громкость принадлежит нажатию (`check_requested`), и `start_full_pass`
/// обязан её спрашивать: иначе потерянный громкий поток заменялся тихим,
/// тот гасил «Проверяю…», отметку кнопки не снимал, и следующий повтор по
/// любому поводу становился громким — `last-check.txt` и десяток строк
/// журнала человеку, который ничего не нажимал.
///
/// Тот же флаг держит «Проверяю…»: пока нажатие ждёт громкого показа,
/// карточка не мигает.
#[test]
fn loudness_belongs_to_the_press_and_the_card_does_not_blink() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.busy = Some(Busy::Check);
    ui.check_requested = true;
    let gen = start_facts(&mut ui, Instant::now()).expect("расчёт пошёл");

    // Тихий ответ пришёл, а кнопке ещё должен громкий показ — «Проверяю…»
    // не гаснет, и отметка кнопки цела.
    apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
    assert_eq!(ui.busy, Some(Busy::Check), "карточка мигнула: «Проверяю…» сняли до отчёта");
    assert!(ui.check_requested, "отметка кнопки съедена, и отчёт достанется чужому повтору");
    // Но отметка полного расчёта безусловна — цикл рвёт именно она.
    assert!(ui.facts_full_at.is_some(), "живая блокировка вернулась");

    // Расчёт, который заводится, пока нажатие не обслужено, — ГРОМКИЙ:
    // отчёт получит он, а не случайный следующий. Громкость видна по
    // `Ev::Ready`, который присылает поток; громкий путь шлёт по дороге
    // ещё и фазу со строками, поэтому ждём именно набор.
    fn loudness_of_the_pass(rx: &mut futures::channel::mpsc::UnboundedReceiver<super::Ev>) -> bool {
        loop {
            match futures::executor::block_on(futures::StreamExt::next(rx)).expect("расчёт не ответил") {
                super::Ev::Ready(_, _, loud) => return loud,
                _ => continue,
            }
        }
    }

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    ui.tx = tx;
    assert!(start_full_pass(&mut ui), "повтор не начался");
    assert!(loudness_of_the_pass(&mut rx), "кнопка нажата, а расчёт пошёл тихим — отчёта человек не увидит");
    assert!(ui.kit.state_path("last-check.txt").exists(), "громкий расчёт отчёта не написал");

    // А без нажатия — тихий, и отчёта нет: такт человеку журнал не сыплет.
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    ui.tx = tx;
    assert!(!ui.check_requested);
    assert!(start_full_pass(&mut ui), "расчёт по такту не начался");
    assert!(!loudness_of_the_pass(&mut rx), "тихий расчёт попросил громкий показ");
    assert!(!ui.kit.state_path("last-check.txt").exists(), "отчёт написан без нажатия кнопки");
}

/// Спека: при выключенном обслуживании вердикт не считается — показывать
/// его негде, а набор стоит до минуты (`verify::SET_DEADLINE`) и
/// поднимает пробу инструментов (на Linux ещё и виртуальный экран).
/// Замок при этом пересчитывается всегда: он решает, доступен ли
/// переключатель (находка S9).
#[test]
fn the_verdict_is_not_computed_while_serving_is_off() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(false, Ordering::Relaxed);
    // «Уже считается», чтобы доказательство не подняло настоящий поток
    // фактов: пересчёт тогда только пометит повтор — эта пометка и
    // показывает, дошло ли до расчёта дело.
    ui.facts_pending = true;
    ui.facts_since = Some(Instant::now());
    ui.facts_again = false;
    ui.tray_shown = None;

    run_readiness(&mut ui);
    assert!(!ui.facts_again, "вердикт считается при выключенном обслуживании");
    assert!(ui.tray_shown.is_some(), "замок обязан пересчитываться и доезжать до значка всегда");

    // Включили — считается само, отдельного события не нужно.
    ui.enabled.store(true, Ordering::Relaxed);
    run_readiness(&mut ui);
    assert!(ui.facts_again, "обслуживание включено, а пересчёт не пошёл");
}

/// Сторож зависшего расчёта (находка S10): бюджет проверяется до факта, а
/// не прерывает его, и поток может не ответить никогда. Залипшая защёлка
/// означала бы вердикт, который больше не посчитается ни разу, — точка
/// навсегда осталась бы «Проверяю…».
#[test]
fn a_lost_readiness_thread_does_not_latch_forever() {
    // Якорь в будущем: на свежей машине `Instant::now() - 5 мин` паникует.
    let now = Instant::now() + Duration::from_secs(3600);
    assert!(!readiness_lost(Some(now), now));
    assert!(!readiness_lost(Some(now), now + READINESS_LOST), "срок ещё не вышел");
    assert!(readiness_lost(Some(now), now + READINESS_LOST + Duration::from_secs(1)));
    assert!(readiness_lost(None, now), "защёлку взвели мимо `start_facts` — ждать нечего");

    // Проводка: пока поток жив, второго не заводим; потерянный — заводим.
    let mut ui = Ui::for_test();
    let gen = start_facts(&mut ui, now).expect("первый расчёт идёт сам");
    assert!(start_facts(&mut ui, now + Duration::from_secs(30)).is_none(), "второй поток поверх живого");
    assert_eq!(start_facts(&mut ui, now + READINESS_LOST + Duration::from_secs(1)), Some(gen + 1));
    // Ответ снимает и защёлку, и отметку времени — иначе следующий
    // расчёт считал бы себя потерянным с рождения.
    apply_facts(&mut ui, gen + 1, Vec::new());
    assert!(!ui.facts_pending && ui.facts_since.is_none());
}

/// Проводка, а не чистая функция (находка S1): вердикт становится
/// известен в `apply_facts`, и оттуда он обязан дойти до значка и до
/// журнала. Раньше `Ev::Ready` звал только перерисовку окна, и
/// установившееся положение дел было «карточка жёлтая, значок зелёный».
#[test]
fn a_verdict_reaches_the_tray_and_the_log() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.link = State::Up { since: Instant::now() };
    // Замок закрыл бы карточку собой (в каталоге теста он заперт), а речь
    // здесь про вердикт.
    ui.gate = None;
    ui.tray_shown = None;
    let gen = ui.facts_gen;
    let fail = Check::fail("проект", "/home/dev3/project не разложен", "1c-connect install");

    let log_before = ui.log.len();
    ui.facts_pending = true;
    assert!(!apply_facts(&mut ui, gen, vec![fail.clone()]));
    let shown = ui.tray_shown.clone().expect("вердикт не дошёл до значка");
    assert_eq!(shown.state, TrayState::Warn, "значок остался прежним, пока карточка жёлтая");
    // И строкой в журнал: подсказка по наведению — не единственный канал
    // (спека называет три, журнал молчал — находка S3).
    assert_eq!(ui.log.len(), log_before + 1, "{:?}", &ui.log[log_before..]);
    let line = ui.log.last().unwrap();
    assert_eq!(line.kind, Kind::Warn);
    assert!(line.text.contains("/home/dev3/project не разложен"), "{}", line.text);

    // Та же причина во второй раз журнал не засыпает: вердикт
    // пересчитывается по каждому событию.
    ui.facts_pending = true;
    apply_facts(&mut ui, gen, vec![fail]);
    assert_eq!(ui.log.len(), log_before + 1, "{:?}", &ui.log[log_before..]);

    // А зелёный вердикт доезжает до значка так же.
    ui.facts_pending = true;
    apply_facts(&mut ui, gen, vec![Check::pass("проект", "в порядке")]);
    assert_eq!(ui.tray_shown.unwrap().state, TrayState::Up);
}

/// Проводка срока (находка S4): мост спрашивает часы на каждый вызов и
/// после полуночи отказывает всем, а окно смотрело в запомненное и
/// показывало зелёное. Смена состояния лицензии — такое же событие,
/// меняющее почву, и кончается тем же пересчётом.
#[test]
fn a_licence_change_recomputes_readiness() {
    let mut ui = Ui::for_test();
    // Запомненное состояние заведомо отличается от настоящего, какое бы
    // оно ни было сегодня.
    let real = crate::licence::state_now();
    ui.licence = if real == crate::licence::State::Expired { crate::licence::State::Valid } else { crate::licence::State::Expired };
    // Обслуживание включено — иначе пересчёт и не начнётся (S9), и
    // доказательство ниже ничего бы не доказало.
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    // Поток фактов в тесте нам не нужен: «уже считается» (и не потеряно,
    // см. `readiness_lost`) заставляет пересчёт только пометить повтор —
    // эта пометка и есть доказательство, что `run_readiness` позвали.
    ui.facts_pending = true;
    ui.facts_since = Some(Instant::now());
    ui.facts_again = false;
    ui.tray_shown = None;

    note_licence_change_ui(&mut ui);

    assert_eq!(ui.licence, real, "новое состояние срока не запомнено");
    assert!(ui.facts_again, "смена срока не привела к пересчёту готовности");
    assert!(ui.tray_shown.is_some(), "пересчёт не дошёл до значка");
}
