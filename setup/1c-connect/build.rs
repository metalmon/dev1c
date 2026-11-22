//! Метка сборки и значок exe.
//!
//! Метка — вторая половина опознания беды по телефону: номер отказа говорит,
//! где сломалось, метка — в каком бинаре. Собирается она здесь, потому что
//! дату, ветку и хеш знает только машина, которая собирает; формат метки —
//! в `common::code`, где его проверяют тесты.
//!
//! Значок exe кладёт embed-resource. Сборка без окна
//! (`--no-default-features`) и сборка не под Windows значка не ставят, а
//! метку получают наравне со всеми: `--version` и отчёт «Проверить» есть и
//! без окна.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    stamp();
    #[cfg(all(windows, feature = "gui"))]
    embed_resource::compile("icons/app.rc", embed_resource::NONE).manifest_optional().unwrap();
}

/// Вывод команды одной строкой или `None`, если она не нашлась, отказала или
/// промолчала. Ни один отказ сборку не валит: комплект собирают и из копии без
/// `.git`, а сборка, которая этого не умеет, не соберётся у того, кто её
/// починил бы.
fn run(prog: &str, dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(prog).current_dir(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// То же про `git`: имя программы повторяется в четырёх местах, и путать его
/// с `dir` на каждом вызове незачем.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    run("git", dir, args)
}

/// Местный день от системы. Не UTC: человек на звонке говорит «двадцать
/// второе», и метка обязана говорить то же — ночная сборка, подписанная
/// вчерашним днём, ломает ровно то, ради чего метка заведена.
///
/// Через подпроцесс, потому что местный сдвиг в std не добыть, а новых крейтов
/// добавлять нельзя. Формат просим машинный (`yyyy-MM-dd`, `%F`), а не
/// локальный: вывод под чужой локалью `parse_ymd` отвергнет целиком, и дорога
/// уйдёт на запасную — соврать датой она не даст.
fn local_ymd(dir: &Path) -> Option<(u64, u64, u64)> {
    let said = if cfg!(windows) {
        run("powershell", dir, &["-NoProfile", "-Command", "(Get-Date).ToString('yyyy-MM-dd')"])
    } else {
        run("date", dir, &["+%F"])
    };
    common::code::parse_ymd(&said?)
}

/// Выдать обе формы метки в `BUILD_STAMP_SHORT` и `BUILD_STAMP_FULL`.
fn stamp() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo задаёт CARGO_MANIFEST_DIR сборочному скрипту");
    let dir = Path::new(&dir);
    let hash = git(dir, &["rev-parse", "--short", "HEAD"]);
    // Отсоединённая голова: `--abbrev-ref` отдаёт само слово `HEAD`, и назвать
    // так ветку значит соврать — имени ветки у такой сборки нет.
    let branch = match git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(name) if name != "HEAD" => name,
        _ => common::code::NO_BRANCH.to_string(),
    };
    watch_git(dir);

    // Команда за датой не вышла — считаем сами от секунд эпохи. Это UTC, и
    // ночью метка отстанет на день; без даты она была бы хуже.
    let (y, m, d) = local_ymd(dir).unwrap_or_else(|| {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or(0);
        common::code::ymd_utc(secs)
    });
    let (short, full) = common::code::stamp_dates(y, m, d);
    println!("cargo:rustc-env=BUILD_STAMP_SHORT={}", common::code::stamp_short(&short, hash.as_deref(), &branch));
    println!("cargo:rustc-env=BUILD_STAMP_FULL={}", common::code::stamp_full(&full, hash.as_deref(), &branch));
}

/// Следить за тем, что меняется при коммите, — иначе метка застрянет на
/// вчерашней, и обещание «видно, какая сборка» станет враньём.
///
/// Пути спрашиваем у самого `git`, а не складываем из `.git/...`: здесь
/// ворктри, и `.git` в нём — файл со ссылкой на общий каталог. `HEAD` такой
/// сборки лежит в `.git/worktrees/<имя>/HEAD`, а файл ветки — в общем
/// `.git/refs/heads/<ветка>`, и меняется при коммите именно он. Угадать эти
/// два места по корню дерева нельзя, `git rev-parse --git-path` их знает.
///
/// `git` не нашёлся — не следим ни за чем: в копии без `.git` следить не за
/// чем, и cargo вернётся к своему умолчанию (перезапуск скрипта на любую
/// правку в пакете).
fn watch_git(dir: &Path) {
    let head = git(dir, &["rev-parse", "--git-path", "HEAD"]);
    // Отсоединённая голова своего файла ветки не имеет — следим только за
    // `HEAD`, он при переходе на другой коммит и меняется.
    let branch_ref = git(dir, &["symbolic-ref", "--quiet", "HEAD"]).and_then(|r| git(dir, &["rev-parse", "--git-path", &r]));
    for path in [head, branch_ref].into_iter().flatten() {
        println!("cargo:rerun-if-changed={path}");
    }
}
