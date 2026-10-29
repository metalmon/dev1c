//! Права каталогов и файлов, которые не должен читать никто кроме владельца:
//! дескриптор публикации несёт пароль базы (`publication.rs`), ключ
//! шифрования — пароль рабочего места и базы (`secrets.rs`). Раньше тот же
//! набор функций жил только в `publication.rs` и для ключа шифрования
//! пришлось бы писать его пятый вариант — вместо этого он один, здесь (ревью
//! задачи 1 модуля секретов, Important 3).

use std::path::Path;

/// Каталог только для владельца: `DirBuilder` с `mode(0o700)` действует
/// только на вновь созданный каталог (обычный `create_dir_all` оставил бы
/// umask, как правило 0755) — если каталог уже есть, вызов тихо ничего не
/// делает, прав существующего каталога НЕ трогает. Это осознанно: у
/// `secrets.rs` есть тест на каталог, у которого права сужены человеком
/// (симуляция внешнего отказа) — поджимать их здесь обратно к 0700 значило
/// бы само снимать это ограничение. Кому нужно ещё и поджать унаследованный
/// от старой версии каталог — зовите `tighten_dir` отдельно (так и делает
/// `publication::ensure_pub_dir`).
#[cfg(unix)]
pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{} не создан: {e}", dir.display()))
}

#[cfg(not(unix))]
pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{} не создан: {e}", dir.display()))
}

/// Поджимает права уже существующего каталога до 0700. Отдельно от
/// `ensure_private_dir` — вызывающий решает сам, уместно ли здесь перебивать
/// права, которые мог сузить кто-то снаружи (см. комментарий выше).
#[cfg(unix)]
pub fn tighten_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("права {} не выставлены: {e}", dir.display()))
}

/// Поджимает права уже существующего файла до 0600 — нужно и после записи
/// (файл мог остаться с прошлого запуска более открытым), и после чтения
/// файла, который мы не писали сами в этом запуске (ключ шифрования,
/// прочитанный, а не созданный заново).
#[cfg(unix)]
pub fn tighten_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("права {} не выставлены: {e}", path.display()))
}

#[cfg(not(unix))]
pub fn tighten_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Текстовый файл с правами только для владельца: на Unix — 0600 одним
/// вызовом `OpenOptions`, чтобы между созданием и правкой прав не было окна
/// (находка автоматической проверки безопасности), и поджатие на случай,
/// если файл остался от прошлого запуска более открытым. На прочих
/// платформах ограничивать нечего — там каталог и так под ACL пользователя.
#[cfg(unix)]
pub fn write_owner_only(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{} не создан: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{} не записан: {e}", path.display()))?;
    tighten_file(path)
}

#[cfg(not(unix))]
pub fn write_owner_only(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{} не записан: {e}", path.display()))
}
