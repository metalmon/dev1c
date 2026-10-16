//! Содержимое флайаута. Заглушка задачи 3: пустой холст, фокус на нём есть,
//! чтобы `Esc` и потеря фокуса (задача 4) было на чём ловить. Вкладки и
//! формы заполнит задача 6.

use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct Panel {
    pub focus: FocusHandle,
}

impl Panel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle() }
    }
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
