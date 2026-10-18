//! Окно: значок exe кладёт embed-resource. Сборка без окна
//! (`--no-default-features`) и сборка не под Windows ничего не делают.
fn main() {
    #[cfg(all(windows, feature = "gui"))]
    embed_resource::compile("icons/app.rc", embed_resource::NONE).manifest_optional().unwrap();
}
