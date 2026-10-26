//! Оконная сборка на Windows не имеет консоли: двойной щелчок не должен
//! открывать чёрное окно. Для команд консоль возвращается сюда — к консоли
//! вызвавшего, а если её нет, к своей. Скопировано из volt-setup.

/// Зовётся первым делом в `main`. Если вывод уже куда-то ведёт (канал, файл,
/// консоль вызвавшего в консольной сборке), его не трогаем: иначе `> файл`
/// сломался бы.
#[cfg(windows)]
pub fn attach() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, SetConsoleCP, SetConsoleOutputCP, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
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

/// Программу запустили двойным щелчком, а не из терминала: консоль наша, и мы
/// в ней одни. Такое окно исчезает в тот же миг, когда программа кончилась, и
/// прочитать хоть строку человек не успевает.
///
/// Из терминала в консоли минимум два процесса — оболочка и мы; в службе и по
/// SSH консоли нет вовсе. Ни там, ни там ждать нечего.
#[cfg(windows)]
pub fn own_window() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    let mut ids = [0u32; 2];
    unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) == 1 }
}

#[cfg(not(windows))]
pub fn own_window() -> bool {
    false
}

/// Задержать окно консоли, если оно закроется вместе с программой. Зовётся
/// последним делом в `main`, и при ошибке тоже: сообщение об ошибке — как раз
/// то, что обиднее всего не успеть прочитать.
pub fn hold_window() {
    if !own_window() {
        return;
    }
    println!();
    println!("Окно закроется, когда вы нажмёте Enter.");
    // Закрытый stdin вернёт ноль сразу: повиснуть здесь нельзя.
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}
