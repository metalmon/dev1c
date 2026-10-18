//! Вид окна. Заготовка: заголовок со своей рамкой; содержимое кладут
//! задачи 4–6 плана.

use gpui_kit::component::{v_flex, TitleBar};
use gpui_kit::*;

pub struct Panel;

impl Panel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Panel
    }
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        v_flex()
            .size_full()
            .bg(t.background)
            .text_color(t.foreground)
            .child(TitleBar::new().child(div().font_weight(FontWeight::SEMIBOLD).child("dev1c-setup — среда разработки 1С")))
    }
}
