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

/// Вывод `git` одной строкой или `None`, если `git` не нашёлся, отказал или
/// промолчал. Отказ `git` сборку не валит: комплект собирают и из копии без
/// `.git`, а сборка, которая этого не умеет, не соберётся у того, кто её
/// починил бы.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").current_dir(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!line.is_empty()).then_some(line)
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
        _ => "сборка".to_string(),
    };
    watch_git(dir);

    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (short, full) = common::code::stamp_dates(secs);
    println!("cargo:rustc-env=BUILD_STAMP_SHORT={}", common::code::stamp_short(&short, &branch));
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
