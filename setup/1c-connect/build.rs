//! Окно: манифест и значок кладёт tauri-build. Сборка без окна
//! (`--no-default-features`) ничего не делает.
fn main() {
    #[cfg(feature = "gui")]
    tauri_build::build();
}
