//! Значок exe кладёт embed-resource. К окну это не привязано: в проводнике
//! значок виден и у консольной программы, а безымянный серый лист рядом с
//! инструкцией заказчика выглядит именно тем, чем кажется.
fn main() {
    #[cfg(windows)]
    embed_resource::compile("icons/app.rc", embed_resource::NONE).manifest_optional().unwrap();
}
