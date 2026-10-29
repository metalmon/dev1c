//! Права каталогов и файлов, которые не должен читать никто кроме владельца:
//! дескриптор публикации несёт путь к базе и состав публикации
//! (`publication.rs`) — не пароль, с задачи 6 его там нет вовсе, но и не
//! чужое дело на образе, где диск делят разные рабочие места; ключ
//! шифрования — пароль рабочего места и базы (`secrets.rs`). Раньше тот же
//! набор функций жил только в `publication.rs` и для ключа шифрования
//! пришлось бы писать его пятый вариант — вместо этого он один, здесь (ревью
//! задачи 1 модуля секретов, Important 3).

use std::path::Path;

/// Каталог только для владельца: `DirBuilder` с `mode(0o700)` действует
/// только на вновь созданный каталог (обычный `create_dir_all` оставил бы
/// umask, как правило 0755) — если каталог уже есть, вызов тихо ничего не
/// делает. Поджатие уже существующего каталога — отдельным шагом,
/// `tighten_dir`, вызывающий зовёт его сам (так и делает
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

/// Снимает у каталога биты группы и остальных (`mode & !0o077`), не трогая
/// владельца и не поднимая его права выше того, что уже есть. Это не
/// «выставить 0700»: `0o755` (унаследованный умолчательный umask, которым
/// каталог мог быть создан до этого фикса — или на стенде, где рабочие
/// места делят один диск, унаследованный от родителя доступ соседнего
/// рабочего места) становится `0o700`, а `0o500` (владелец сузил права сам
/// — тест на внешний отказ, `secrets::tests::a_read_only_state_dir_is_a_named_refusal`)
/// остаётся `0o500`: снимать там нечего, и поднимать владельца до `rwx` мы
/// не вправе — это решение того, кто сузил права, не наше. Безусловное
/// `set_permissions(0o700)` здесь было бы ошибкой в обе стороны сразу: в
/// тесте — тихо снимало бы искусственное ограничение, а на стенде —
/// оставляло бы ровно ту дыру, которую и чинит эта функция (ревью задачи 1,
/// круг 3, пункт 1): унаследованные права группы/остальных на каталоге
/// ключа — путь, которым соседний пользователь может прочитать или
/// подменить `secret.key` (тот же Critical 1, только со стороны каталога).
#[cfg(unix)]
pub fn tighten_dir(dir: &Path) -> Result<(), String> {
    narrow_to_owner(dir)
}

#[cfg(not(unix))]
pub fn tighten_dir(_dir: &Path) -> Result<(), String> {
    Ok(())
}

/// То же сужение для файла: ключ, прочитанный не нами созданным в этом
/// запуске, не должен остаться читаемым (или перезаписываемым) соседом по
/// той же причине, что и каталог выше.
#[cfg(unix)]
pub fn tighten_file(path: &Path) -> Result<(), String> {
    narrow_to_owner(path)
}

#[cfg(not(unix))]
pub fn tighten_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn narrow_to_owner(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).map_err(|e| format!("права {} не прочитаны: {e}", path.display()))?.permissions().mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & !0o077))
        .map_err(|e| format!("права {} не выставлены: {e}", path.display()))
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

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fsperm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    /// Контракт, из-за которого исполнитель один раз уже отказался от
    /// поджатия, а потом вернул его с другой семантикой (ревью задачи 1,
    /// круг 3): сузить искусственно расширенное, не расширить искусственно
    /// суженное. Записан тестом, а не выведен из `publication`/`secrets`.
    #[test]
    fn tighten_dir_narrows_group_and_other_but_never_widens_the_owner() {
        let dir = tmp("tighten-dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        tighten_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700, "0o777 должен сузиться до 0o700");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        tighten_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o500, "0o500 — решение владельца сузить права самому, поджатие его не трогает");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tighten_file_narrows_group_and_other_the_same_way() {
        let dir = tmp("tighten-file");
        let path = dir.join("f");
        std::fs::write(&path, "x").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        tighten_file(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600, "0o644 должен сузиться до 0o600");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ensure_private_dir` создаёт новый каталог с 0700 сама — и не трогает
    /// режим уже существующего (это работа `tighten_dir`, не её).
    #[test]
    fn ensure_private_dir_sets_0700_on_creation_and_leaves_an_existing_dir_alone() {
        let dir = tmp("ensure");
        let _ = std::fs::remove_dir_all(&dir);
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700, "вновь созданный каталог обязан получить 0700, а не umask");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o750, "каталог уже есть — ensure_private_dir его режим не меняет");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
