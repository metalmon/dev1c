//! Выгрузка и загрузка: те же инструменты, что у агента, в том же порядке
//! и под тем же мьютексом. Чистая часть — через `Deps`, чтобы проверять
//! порядок шагов без платформы.

use crate::local::Reply;
use crate::ops_host::Ctx;
use common::{Phase, Runner};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub struct Deps<'a> {
    pub call: &'a dyn Fn(&str, Value) -> Result<Reply, String>,
    pub platform_running: &'a dyn Fn() -> usize,
    pub push: &'a dyn Fn(&str) -> Result<String, String>,
    pub pull: &'a dyn Fn() -> Result<String, String>,
    pub dump_index_exists: bool,
    /// `--dry-run`: только план, ни одного вызова.
    pub dry_run: bool,
    pub phase: &'a dyn Fn(Phase),
    /// Пишет `.dev1c-dump` рядом с деревом после выгрузки (версия платформы
    /// и версия формата). Ошибка не валит выгрузку — файлы уже выгружены и
    /// отправлены важнее отметки, поэтому только предупреждение в журнал.
    pub write_mark: &'a dyn Fn() -> Result<(), String>,
    /// Сверка версии формата дерева с версией нашей платформы перед
    /// загрузкой. `None` — сверить нечем (отметки нет, или не нашли свою
    /// платформу/её формат); это не отказ, просто сверка не идёт.
    pub mark_check: &'a dyn Fn() -> Option<Mark>,
}

/// Имя файла-отметки о платформе и формате выгрузки рядом с деревом.
/// В `.gitignore` проекта его не добавляют: он мал и нужен второй стороне.
const DUMP_MARK_FILE: &str = ".dev1c-dump";

/// Версия платформы из пути к `1cv8.exe`, который возвращает
/// `ops_host::find_1cv8()`: имя каталога двумя уровнями выше исполняемого
/// файла (`.../8.5.1.1150/bin/1cv8.exe` → `8.5.1.1150`).
pub(crate) fn platform_version_from_exe(exe: &Path) -> Option<String> {
    exe.parent()?.parent()?.file_name()?.to_str().map(str::to_string)
}

/// Версия нашей платформы — через найденный `1cv8.exe`.
fn our_platform_version(cfg: &crate::cfg::Cfg) -> Option<String> {
    platform_version_from_exe(&crate::ops_host::find_1cv8(cfg)?)
}

/// Версия формата выгрузки, которую производит сборка платформы — короткая
/// таблица по примечаниям к выпускам платформы и по замеру на этой машине
/// (design-док, «Отладка в образе на учебной платформе»): 8.3.20 → 2.13,
/// 8.3.21 → 2.14, 8.3.27 → 2.20, 8.5.1 → 2.21, 8.5.4 → 2.22. Платформа новее
/// последней строки или вовсе не из таблицы — формат не знаем, сверку перед
/// загрузкой тогда пропускаем: это безопаснее угадывания.
/// ponytail: маленькая таблица вместо разбора схемы формата из самой
/// платформы (`backendt_root.res`, пакет `xcf/dumpinfo`); дополнить строкой
/// при новом выпуске платформы, или перейти на чтение схемы, если таблица
/// станет неудобной.
fn format_for_platform(platform: &str) -> Option<&'static str> {
    let short = platform.splitn(4, '.').take(3).collect::<Vec<_>>().join(".");
    Some(match short.as_str() {
        "8.3.20" => "2.13",
        "8.3.21" => "2.14",
        "8.3.27" => "2.20",
        "8.5.1" => "2.21",
        "8.5.4" => "2.22",
        _ => return None,
    })
}

/// Читает отметку `.dev1c-dump` в каталоге проекта: версия платформы и
/// версия формата, которыми дерево было выгружено. `None`, если отметки нет
/// или она повреждена (например, дерево пришло от другой машины через
/// Gitea, минуя наш `export`).
pub fn dump_mark(project: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(project.join(DUMP_MARK_FILE)).ok()?;
    let mut platform = None;
    let mut format = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("platform=") {
            platform = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("format=") {
            format = Some(v.trim().to_string());
        }
    }
    Some((platform?, format?))
}

/// Пишет отметку `.dev1c-dump` рядом с деревом: версия платформы, которая
/// выгружала, и версия формата выгрузки.
pub fn write_dump_mark(project: &Path, platform: &str, format: &str) -> Result<(), String> {
    std::fs::write(project.join(DUMP_MARK_FILE), format!("platform={platform}\nformat={format}\n"))
        .map_err(|e| format!("не удалось записать отметку о платформе ({DUMP_MARK_FILE}): {e}"))
}

/// Версия формата выгрузки — атрибут `version` корневого элемента
/// `ConfigDumpInfo` в `src/ConfigDumpInfo.xml`. Дерево — десятки тысяч
/// файлов, поэтому читаем только этот файл и только его начало текстом, не
/// XML-парсером; файл начинается с BOM, его снимаем перед поиском.
pub fn dump_format(project: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = [0u8; 512];
    let mut f = std::fs::File::open(project.join("src").join("ConfigDumpInfo.xml")).ok()?;
    let n = f.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let head = head.strip_prefix('\u{feff}').unwrap_or(&head);
    let tail = &head[head.find("ConfigDumpInfo")?..];
    let tail = &tail[tail.find("version=\"")? + "version=\"".len()..];
    Some(tail[..tail.find('"')?].to_string())
}

/// Версия формата покомпонентно как числа: `2.9` старее `2.20` (текстовое
/// сравнение строк тут путает: "2.9" > "2.20" как строки).
fn parse_format(v: &str) -> Vec<u32> {
    v.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// Итог сверки версии формата дерева с версией нашей платформы.
#[derive(Debug, PartialEq, Eq)]
pub enum Mark {
    /// Версии совпадают — грузим молча.
    Same,
    /// Дерево старее нашей платформы — грузится с предупреждением в журнал.
    Older(String),
    /// Дерево новее нашей платформы — платформа такой формат не прочитает,
    /// загрузка не начинается.
    Newer(String),
}

/// Чистая функция сверки — без файлов, без платформы, тесты проверяют
/// именно её.
pub fn check_mark(tree_format: Option<&str>, ours: Option<&str>) -> Mark {
    let (Some(t), Some(o)) = (tree_format, ours) else {
        return Mark::Same;
    };
    match parse_format(t).cmp(&parse_format(o)) {
        Ordering::Equal => Mark::Same,
        Ordering::Less => Mark::Older(format!(
            "файлы выгружены в формате {t}, ваша платформа работает с форматом {o}; после выгрузки из этой базы дерево станет формата {o}, и платформа с форматом {t} его уже не примет"
        )),
        Ordering::Greater => Mark::Newer(format!(
            "эти файлы выгружены в формате {t}, ваша платформа читает только до формата {o} — загрузить нельзя"
        )),
    }
}

fn lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    match crate::project::LOCK.try_lock() {
        Ok(g) => Ok(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Ok(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => {
            let what = crate::project::doing();
            Err(format!("занято: идёт {}", if what.is_empty() { "другая операция" } else { what }))
        }
    }
}

/// cf_decompile в src, затем project_push.
/// Строки хода работы не обещают времени. Прежние («минуты», «десятки тысяч
/// файлов») писались по УНФ и стали неправдой на демонстрационной базе
/// стенда: там 844 файла и секунды (замер 2026-10-21). Сколько займёт у
/// человека, зависит от его конфигурации — от секунд на маленькой до минут
/// на УНФ и дольше на ЕРП, — и назвать одно число нельзя. Сколько идёт на
/// самом деле, человек видит секундомером в карточке окна, а после —
/// строкой «Готово за …».
pub fn export_with(d: &Deps, message: &str, log: &dyn Fn(String)) -> Result<String, String> {
    let _g = lock()?;
    if d.dry_run {
        log("план: cf_decompile → src, затем git add/commit/push ветки рабочего места (project_push)".into());
        return Ok("план напечатан, на машине ничего не менялось".into());
    }
    (d.phase)(Phase::Decompile);
    log("выгружаю конфигурацию из 1С в файлы проекта, Конфигуратор работает в фоне (cf_decompile → src)".into());
    let r = (d.call)("cf_decompile", json!({"OUT": "src", "background": false}))?;
    if r.is_error {
        return Err(format!("выгрузка не удалась: {}", r.text));
    }
    if !r.text.trim().is_empty() {
        log(r.text);
    }
    if let Err(e) = (d.write_mark)() {
        log(format!("не удалось записать отметку о платформе рядом с выгрузкой: {e}"));
    }
    (d.phase)(Phase::Push);
    log("отправляю файлы агенту (git add, commit, push в Gitea)".into());
    (d.push)(message)
}

/// project_pull, затем cf_load инкрементом.
pub fn import_with(d: &Deps, log: &dyn Fn(String)) -> Result<String, String> {
    let _g = lock()?;
    if d.dry_run {
        log("план: project_pull (git pull --rebase), затем cf_load --increment из src".into());
        return Ok("план напечатан, на машине ничего не менялось".into());
    }
    let n = (d.platform_running)();
    if n > 0 {
        return Err(format!("на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"));
    }
    (d.phase)(Phase::Pull);
    log((d.pull)()?);
    if !d.dump_index_exists {
        log("src/ConfigDumpInfo.xml нет — загрузка будет полной, не инкрементом (первая после клонирования)".into());
    }
    match (d.mark_check)() {
        None => log("отметки о платформе нет, проверить формат не могу".into()),
        Some(Mark::Same) => {}
        Some(Mark::Older(msg)) => log(format!("предупреждение: {msg}")),
        Some(Mark::Newer(msg)) => return Err(msg),
    }
    (d.phase)(Phase::Load);
    log("загружаю конфигурацию в 1С из файлов проекта (cf_load ← src)".into());
    let r = (d.call)("cf_load", json!({"SRC": "src", "background": false, "increment": true}))?;
    if r.is_error {
        return Err(format!(
            "загрузка не удалась: {} — в Конфигураторе: Конфигурация → Вернуться к конфигурации БД, затем повторите; ветка уже обновлена",
            r.text
        ));
    }
    Ok(if r.text.trim().is_empty() { "загружено".into() } else { r.text })
}

/// Настоящие части: процесс `vrunner-mcp` из инструментов на машине, git проекта.
fn with_real<R>(ctx: &Ctx, cancel: &AtomicBool, f: impl FnOnce(&Deps, &dyn Fn(String)) -> R) -> R {
    let tools = crate::ops_host::tools_dir();
    let p = crate::project::Project::from_cfg(&ctx.cfg, &tools);
    let log = |s: String| ctx.say(s);
    let cfg = ctx.cfg.clone();
    let call = |tool: &str, mut args: Value| {
        // The same connection arguments the bridge injects for the agent.
        if let (Some(a), Some(d)) = (args.as_object_mut(), crate::bridge::connection_args(&cfg).as_object()) {
            for (k, v) in d {
                a.insert(k.clone(), v.clone());
            }
        }
        let cmd = crate::bridge::vrunner_command(&cfg, &tools).into_std();
        crate::local::call(cmd, tool, args, std::time::Duration::from_secs(3600), &log)
    };
    let push = |m: &str| crate::project::push(&p, m, cancel);
    let pull = || crate::project::pull(&p, cancel);
    let phase = |ph: Phase| ctx.progress(ph);
    let write_mark = || -> Result<(), String> {
        // Своей версии платформы или формата не нашли — отметку не пишем
        // молча; файлы уже выгружены и отправлены, это важнее отметки.
        let Some(platform) = our_platform_version(&ctx.cfg) else { return Ok(()) };
        let Some(format) = dump_format(&p.dir) else { return Ok(()) };
        write_dump_mark(&p.dir, &platform, &format)
    };
    let mark_check = || -> Option<Mark> {
        let (tree_platform, tree_format) = dump_mark(&p.dir)?;
        let our_platform = our_platform_version(&ctx.cfg)?;
        let our_fmt = format_for_platform(&our_platform)?;
        Some(match check_mark(Some(&tree_format), Some(our_fmt)) {
            Mark::Same => Mark::Same,
            Mark::Older(_) => Mark::Older(format!(
                "файлы выгружены платформой {tree_platform} (формат {tree_format}), ваша платформа {our_platform} (формат {our_fmt}); после выгрузки из этой базы дерево станет формата {our_fmt}, и платформа {tree_platform} его больше не примет"
            )),
            Mark::Newer(_) => Mark::Newer(format!(
                "эти файлы выгружены платформой {tree_platform} (формат {tree_format}), ваша база работает на {our_platform} (формат {our_fmt}) — загрузить нельзя"
            )),
        })
    };
    let d = Deps {
        call: &call,
        platform_running: &crate::bridge::configurator_count,
        push: &push,
        pull: &pull,
        dump_index_exists: p.dir.join("src").join("ConfigDumpInfo.xml").exists(),
        dry_run: ctx.dry_run,
        phase: &phase,
        write_mark: &write_mark,
        mark_check: &mark_check,
    };
    f(&d, &log)
}

pub fn export(ctx: &Ctx, message: &str, cancel: &AtomicBool) -> Result<String, String> {
    with_real(ctx, cancel, |d, log| export_with(d, message, log))
}

pub fn import(ctx: &Ctx, cancel: &AtomicBool) -> Result<String, String> {
    with_real(ctx, cancel, |d, log| import_with(d, log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<String>>,
        reply: RefCell<Vec<(String, bool)>>, // answers in order
        running: usize,
        pushed: RefCell<bool>,
        pulled: RefCell<bool>,
        index: bool,
        dry: bool,
        phases: RefCell<Vec<Phase>>,
    }
    impl Fake {
        fn new(replies: &[(&str, bool)]) -> Fake {
            Fake {
                calls: RefCell::new(vec![]),
                reply: RefCell::new(replies.iter().map(|(t, e)| (t.to_string(), *e)).collect()),
                running: 0,
                pushed: RefCell::new(false),
                pulled: RefCell::new(false),
                index: true,
                dry: false,
                phases: RefCell::new(vec![]),
            }
        }
        fn run<R>(&self, f: impl FnOnce(&Deps) -> R) -> R {
            let call = |tool: &str, args: Value| {
                self.calls.borrow_mut().push(format!("{tool} {args}"));
                let (t, e) = self.reply.borrow_mut().remove(0);
                Ok(Reply { text: t, is_error: e })
            };
            let platform_running = || self.running;
            let push = |m: &str| {
                *self.pushed.borrow_mut() = true;
                Ok(format!("отправлено: {m}"))
            };
            let pull = || {
                *self.pulled.borrow_mut() = true;
                Ok("pulled".to_string())
            };
            let phase = |ph: Phase| self.phases.borrow_mut().push(ph);
            let write_mark = || -> Result<(), String> { Ok(()) };
            // По умолчанию в тестах отметки нет — ровно сценарий
            // `a_missing_mark_is_not_a_failure`: сверить нечем, но загрузка
            // не падает.
            let mark_check = || -> Option<Mark> { None };
            f(&Deps {
                call: &call,
                platform_running: &platform_running,
                push: &push,
                pull: &pull,
                dump_index_exists: self.index,
                dry_run: self.dry,
                phase: &phase,
                write_mark: &write_mark,
                mark_check: &mark_check,
            })
        }
    }

    /// `export_with`/`import_with` take `project::LOCK` as their first line
    /// (`lock()` above, even on the `dry_run` branch) — cargo runs this
    /// module's tests in parallel in one binary, so any overlap makes the
    /// loser see «занято» and panic on `.unwrap()`. One mutex serialises
    /// them; production code is untouched.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::project::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn export_decompiles_then_pushes() {
        let _s = serial();
        let f = Fake::new(&[("разобрано", false)]);
        let r = f.run(|d| export_with(d, "выгрузка", &|_| {})).unwrap();
        assert!(r.contains("отправлено"), "{r}");
        assert_eq!(f.calls.borrow()[0], r#"cf_decompile {"OUT":"src","background":false}"#);
        assert!(*f.pushed.borrow());
    }

    #[test]
    fn a_failed_decompile_does_not_push() {
        let _s = serial();
        let f = Fake::new(&[("платформа: ошибка", true)]);
        let e = f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert!(e.contains("платформа") && !*f.pushed.borrow(), "{e}");
    }

    #[test]
    fn import_refuses_before_pulling_while_the_platform_runs() {
        let _s = serial();
        let mut f = Fake::new(&[]);
        f.running = 1;
        let e = f.run(|d| import_with(d, &|_| {})).unwrap_err();
        assert!(e.contains("закройте Конфигуратор") && !*f.pulled.borrow(), "{e}");
    }

    #[test]
    fn import_pulls_then_loads_incrementally_and_warns_without_the_index() {
        let _s = serial();
        let mut f = Fake::new(&[("загружено", false)]);
        f.index = false;
        let log = RefCell::new(Vec::new());
        let r = f.run(|d| import_with(d, &|s| log.borrow_mut().push(s))).unwrap();
        assert!(*f.pulled.borrow() && r.contains("загружено"), "{r}");
        assert_eq!(f.calls.borrow()[0], r#"cf_load {"SRC":"src","background":false,"increment":true}"#);
        assert!(log.borrow().iter().any(|l| l.contains("ConfigDumpInfo.xml")), "{:?}", log.borrow());
    }

    #[test]
    fn failed_load_names_the_way_back() {
        let _s = serial();
        let f = Fake::new(&[("Конфигуратор: ошибка загрузки", true)]);
        let e = f.run(|d| import_with(d, &|_| {})).unwrap_err();
        assert!(e.contains("Вернуться к конфигурации БД") && *f.pulled.borrow(), "{e}");
    }

    #[test]
    fn dry_run_prints_the_plan_and_touches_nothing() {
        // --dry-run is global and promises «ничего не делать»: no decompile,
        // no push, no pull, no load — only the plan in the log.
        let _s = serial();
        let mut f = Fake::new(&[]);
        f.dry = true;
        let log = RefCell::new(Vec::new());
        let r = f.run(|d| export_with(d, "x", &|s| log.borrow_mut().push(s))).unwrap();
        assert!(f.calls.borrow().is_empty() && !*f.pushed.borrow(), "{r}");
        assert!(log.borrow().iter().any(|l| l.contains("cf_decompile")), "{:?}", log.borrow());
        let r = f.run(|d| import_with(d, &|_| {})).unwrap();
        assert!(f.calls.borrow().is_empty() && !*f.pulled.borrow(), "{r}");
    }

    #[test]
    fn the_git_phase_of_export_is_announced_before_it_starts() {
        let _s = serial();
        let f = Fake::new(&[("разобрано", false)]);
        let log = RefCell::new(Vec::new());
        f.run(|d| export_with(d, "m", &|s| log.borrow_mut().push(s))).unwrap();
        let log = log.borrow();
        // Человек должен узнать про отправку до того, как она пойдёт: вторая
        // половина выгрузки дольше первой и выглядит как зависание, если о
        // ней не сказать.
        assert!(log.iter().any(|l| l.contains("отправляю") && l.contains("git")), "{log:?}");
        // И не обещать при этом времени: прежние «минуты» писались по УНФ, а
        // на демонстрационной базе стенда это секунды (замер 2026-10-21).
        assert!(!log.iter().any(|l| l.contains("минут")), "строки хода работы не обещают времени: {log:?}");
    }

    #[test]
    fn export_is_told_busy_like_any_tool() {
        let _s = serial();
        let _g = crate::project::LOCK.try_lock().unwrap_or_else(|e| match e {
            std::sync::TryLockError::Poisoned(g) => g.into_inner(),
            e => panic!("{e}"),
        });
        let f = Fake::new(&[]);
        let e = f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert!(e.contains("занято"), "{e}");
    }

    #[test]
    fn export_announces_decompile_then_push() {
        let _s = serial();
        let f = Fake::new(&[("разобрано", false)]);
        f.run(|d| export_with(d, "m", &|_| {})).unwrap();
        assert_eq!(*f.phases.borrow(), vec![Phase::Decompile, Phase::Push]);
    }

    #[test]
    fn import_announces_pull_then_load() {
        let _s = serial();
        let f = Fake::new(&[("загружено", false)]);
        f.run(|d| import_with(d, &|_| {})).unwrap();
        assert_eq!(*f.phases.borrow(), vec![Phase::Pull, Phase::Load]);
    }

    #[test]
    fn dry_run_announces_nothing() {
        let _s = serial();
        let mut f = Fake::new(&[]);
        f.dry = true;
        f.run(|d| export_with(d, "x", &|_| {})).unwrap();
        f.run(|d| import_with(d, &|_| {})).unwrap();
        assert!(f.phases.borrow().is_empty(), "{:?}", f.phases.borrow());
    }

    #[test]
    fn a_failed_decompile_announces_no_push() {
        let _s = serial();
        let f = Fake::new(&[("платформа: ошибка", true)]);
        f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert_eq!(*f.phases.borrow(), vec![Phase::Decompile]);
    }

    #[test]
    fn same_version_loads_silently() {
        assert_eq!(check_mark(Some("2.21"), Some("2.21")), Mark::Same);
    }

    #[test]
    fn older_files_load_with_a_warning() {
        let Mark::Older(msg) = check_mark(Some("2.20"), Some("2.21")) else {
            panic!("ожидали Mark::Older")
        };
        assert!(msg.contains("2.20") && msg.contains("2.21") && msg.contains("не примет"), "{msg}");
    }

    #[test]
    fn newer_files_are_refused() {
        // Это тот же отказ, что платформа даёт текстом «Неизвестная версия
        // формата 2.21», но мы ловим его до запуска конфигуратора.
        let Mark::Newer(msg) = check_mark(Some("2.21"), Some("2.20")) else {
            panic!("ожидали Mark::Newer")
        };
        assert!(msg.contains("2.21") && msg.contains("2.20") && msg.contains("нельзя"), "{msg}");
    }

    #[test]
    fn format_versions_compare_as_numbers() {
        // Текстом "2.9" > "2.20" (девятка больше двойки в начале), а по
        // номерам формата 2.9 старее 2.20 — покомпонентное сравнение.
        assert!(matches!(check_mark(Some("2.9"), Some("2.20")), Mark::Older(_)));
    }

    #[test]
    fn a_missing_mark_is_not_a_failure() {
        let _s = serial();
        let f = Fake::new(&[("загружено", false)]);
        let log = RefCell::new(Vec::new());
        let r = f.run(|d| import_with(d, &|s| log.borrow_mut().push(s))).unwrap();
        assert!(r.contains("загружено"), "{r}");
        assert!(log.borrow().iter().any(|l| l.contains("отметки о платформе нет") && l.contains("формат")), "{:?}", log.borrow());
    }

    #[test]
    fn dump_format_reads_the_version_through_the_bom() {
        let dir = std::env::temp_dir().join(format!("dev1c-flow-test-{:?}", std::thread::current().id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("ConfigDumpInfo.xml"),
            "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ConfigDumpInfo xmlns=\"http://v8.1c.ru/8.3/xcf/dumpinfo\" format=\"Hierarchical\" version=\"2.21\">\n",
        )
        .unwrap();
        assert_eq!(dump_format(&dir).as_deref(), Some("2.21"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_dump_mark_round_trips() {
        let dir = std::env::temp_dir().join(format!("dev1c-flow-test-mark-{:?}", std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_dump_mark(&dir, "8.5.1.1150", "2.21").unwrap();
        assert_eq!(dump_mark(&dir), Some(("8.5.1.1150".to_string(), "2.21".to_string())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
