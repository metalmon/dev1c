//! Оконная сборка на Windows не имеет консоли: двойной щелчок не должен
//! открывать чёрное окно. Для команд консоль возвращается сюда — к консоли
//! вызвавшего, а если её нет, к своей. Скопировано из volt-setup.

/// Зовётся первым делом в `main`. Если вывод уже куда-то ведёт (канал, файл,
/// консоль вызвавшего в консольной сборке), его не трогаем: иначе `> файл`
/// сломался бы.
#[cfg(windows)]
pub fn attach() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, SetConsoleCP, SetConsoleOutputCP, SetStdHandle,
        ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let no_output = out.is_null() || out == INVALID_HANDLE_VALUE;
        if std::env::args().len() > 1 && no_output {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            let h = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            if h != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_OUTPUT_HANDLE, h);
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
    }
}

#[cfg(not(windows))]
pub fn attach() {}
