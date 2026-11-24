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

/// Свой каталог во временном, который никто кроме нас не мог подготовить
/// заранее, — и убираемый за собой в любом исходе, панику включая.
///
/// **Почему не `ensure_private_dir` рядом и не `tempdir::Dir` из тестов.**
/// Оба заводят каталог «рекурсивно» (`create_dir_all`), а это значит: уже
/// существующий путь — не отказ, а тихое согласие работать в нём, и
/// символическую ссылку по этому пути вызов пройдёт насквозь. Имя у
/// `tempdir::Dir` вдобавок предсказуемо (номер процесса и время). На Windows
/// временный каталог у каждой учётной записи свой, и там это почти
/// безобидно; на Linux `/tmp` общий — сосед, знающий имя, подкладывает по
/// нему ссылку, и `remove_dir_all` сносит то, на что она указывает, а потом
/// мы ещё и пишем туда (находка проверки безопасности 24.11 по
/// `ops_host::configuration_name`, где выгрузка — десятки мегабайт).
///
/// **Чем неверное сделано невозможным, а не маловероятным.** Имя случайно
/// (`RandomState` — семя от операционной системы, не счётчик и не время), а
/// заводится каталог НЕрекурсивным `create_dir`: он атомарен и на уже
/// существующем пути — ссылка это или настоящий каталог — отдаёт
/// `AlreadyExists`. Повторной попытки нет нарочно: отказ словами честнее
/// обхода, а угадать случайное имя соседу не из чего. Права даёт сам
/// `DirBuilder` (`mode(0o700)`) — выставить их отдельным шагом после
/// создания значило бы снова оставить окно между проверкой и действием.
pub struct PrivateDir(std::path::PathBuf);

impl PrivateDir {
    /// `prefix` — только для человека, который увидит каталог в журнале или
    /// глазами: он говорит, чей это каталог, а защищает не он, а случайный
    /// хвост.
    pub fn new(prefix: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("{prefix}-{:016x}", random_tag()));
        create_dir_private(&dir)?;
        Ok(PrivateDir(dir))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Случайный хвост имени без новой зависимости: `RandomState` сеется
/// операционной системой (`HashMap` защищается им от подобранных ключей —
/// ровно та же нужда, что здесь), и у каждого вызова он свой.
fn random_tag() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(std::process::id() as usize);
    h.finish()
}

#[cfg(unix)]
fn create_dir_private(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir).map_err(|e| format!("{} не создан: {e}", dir.display()))
}

/// На Windows прав каталога мы не правим нигде (`tighten_dir` там пуст — см.
/// его), а временный каталог и без того свой у каждой учётной записи; от
/// подмены пути охраняет то же, что и на Unix: случайное имя и
/// нерекурсивный `create_dir`, который на существующем пути отказывает.
#[cfg(not(unix))]
fn create_dir_private(dir: &Path) -> Result<(), String> {
    std::fs::create_dir(dir).map_err(|e| format!("{} не создан: {e}", dir.display()))
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

/// Эти проверки не про права, а про САМО ЗАВЕДЕНИЕ каталога, и потому идут
/// на обеих осях: именно его правило (случайное имя, отказ на существующем
/// пути, уборка за собой) чинило находку 24.11, а под Windows коннектор
/// собирается ровно так же, как под Linux.
#[cfg(test)]
mod scratch_tests {
    use super::*;

    /// Два каталога подряд — разные, оба существуют, и после `drop` от них не
    /// остаётся ничего. Уборка здесь не вежливость: выгрузка, ради которой
    /// каталог заводится, весит десятки мегабайт.
    #[test]
    fn a_private_scratch_dir_is_new_each_time_and_goes_away() {
        let (a, b) = (PrivateDir::new("fsperm-scratch").unwrap(), PrivateDir::new("fsperm-scratch").unwrap());
        let (pa, pb) = (a.path().to_path_buf(), b.path().to_path_buf());
        assert_ne!(pa, pb, "имя обязано быть случайным, а не предсказуемым");
        assert!(pa.is_dir() && pb.is_dir());
        std::fs::write(pa.join("x"), "x").unwrap();
        drop(a);
        drop(b);
        assert!(!pa.exists() && !pb.exists(), "каталог с содержимым обязан убираться целиком");
    }

    /// Сердце находки: путь, который уже существует, — ОТКАЗ, а не повод
    /// работать в чужом каталоге. Без этого соседу хватило бы угадать имя,
    /// чтобы подставить нам свой каталог (или ссылку на чей-то чужой) —
    /// `create_dir_all` прошёл бы его молча.
    #[test]
    fn an_existing_path_is_a_refusal_not_a_reuse() {
        let taken = PrivateDir::new("fsperm-taken").unwrap();
        let err = create_dir_private(taken.path()).expect_err("существующий путь обязан отказать");
        assert!(err.contains(&taken.path().display().to_string()), "{err}");
    }
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

    /// Права `PrivateDir` даёт сам `DirBuilder`, а не отдельный шаг после
    /// создания: на общем `/tmp` каталог, который существует хотя бы миг с
    /// умолчательным `0o755`, — то самое окно, которого эта проверка и не
    /// позволяет вернуть.
    #[test]
    fn a_private_scratch_dir_is_owner_only_from_the_moment_it_exists() {
        let d = PrivateDir::new("fsperm-mode").unwrap();
        assert_eq!(mode_of(d.path()), 0o700);
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
