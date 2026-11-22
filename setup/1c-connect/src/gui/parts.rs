//! Конструктор окна: примитивы, из которых собираются все экраны. Набор согласован владельцем 10.11.2026.
//!
//! **Зачем он есть.** До него каждое место рисовало себя само, и одна и та же ошибка возвращалась в соседнем месте: кнопка то во всю
//! ширину, то короткая посреди строки; выпадающий список то шире своей кнопки, то уже; пункт меню то откладывает действие, то нет — и
//! тогда вопрос подтверждения получал «да» от того же щелчка. Владелец назвал это прямо: «чтобы не в каждом месте по новой отрисовывать
//! и на те же грабли наступать, а из конструктора потом собирать».
//!
//! **Правило каждого примитива живёт ВНУТРИ него.** Не в договорённости и не в докстроке соседа: `button_wide` не умеет иной ширины,
//! `menu_item` не умеет неотложенного действия, `field_select` с одним вариантом рисует строку, а не список. Неверное становится
//! невыразимым — тот же приём, которым на этой ветке семь раз лечили «две меры на один вопрос».
//!
//! **Имена начинаются с семьи** (решение владельца 10.11: «начинай все с button, тоже за правило хорошего тона возьми»): `button_wide`,
//! `button_icon`, `field_select`, `field_icon`, `menu_item`. Семья первым словом собирает родню рядом в любом списке.
//!
//! Размеры и цвета — только постоянными и из темы: литералов цвета здесь нет.

use crate::gui::facts::Dot;
use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex, Icon, Sizable, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Ширина СПИСКА: окно (`window::MIN_SIZE.width`) минус его отступ (`p_4` — по 16) и место под полосу прокрутки (`pr_2` — 8).
/// По ней стоят строки соединений и «+ Новое соединение», и по ней же — выпадающие списки их кнопок.
pub const LIST_W: Pixels = px(340.);

/// Ширина ПОЛЯ карточки: ширина списка минус отступ карточки (`p_3` — по 12) и её рамка. По ней стоят поля и списки селекторов.
pub const FIELD_W: Pixels = px(314.);

/// Что сделать по выбору ответа в селекторе. Отдельным именем, потому что живёт в списке и копируется в каждый пункт.
pub type Act = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Где стоит кнопка, чей список мы раскрываем. Ширина списка берётся отсюда, а не подбирается на глаз: список, который уже или шире
/// своей кнопки, читается как чужой предмет, случайно оказавшийся рядом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Кнопка стоит в списке: строка соединения, «+ Новое соединение», подвал.
    List,
    /// Кнопка — поле карточки: селектор.
    Field,
}

impl Slot {
    pub fn width(self) -> Pixels {
        match self {
            Slot::List => LIST_W,
            Slot::Field => FIELD_W,
        }
    }
}

/// Цвет точки состояния. ОДИН словарь на всё окно: шапку, строки списка, факты.
pub fn dot(d: Dot, t: &Theme) -> Hsla {
    match d {
        Dot::Off => t.muted_foreground,
        Dot::Busy => t.primary,
        Dot::Up => t.success,
        Dot::Down => t.danger,
        Dot::Warn => t.warning,
    }
}

/// Слой Fluent: заливка, скругление, рамка в 1 px низкой непрозрачности, без тени.
pub fn card(bg: Hsla, border: Hsla) -> Div {
    div().rounded_lg().bg(bg).border_1().border_color(border.opacity(0.6))
}

/// Кнопка во всю ширину — ЕДИНСТВЕННАЯ форма кнопки в карточке, кроме значка справа от поля (закон окна, «Кнопки в карточке: ровно две
/// формы»). Короткая кнопка посреди строки — третья форма, которой не бывает: из-за неё карточка и читалась «как будто школьник делал».
pub fn button_wide(id: impl Into<ElementId>, label: impl Into<SharedString>) -> KitButton {
    KitButton::new(id).outline().w_full().label(label)
}

/// Кнопка-значок: меню строки, копирование, выбор папки, раскрытие карточки.
///
/// Видна ВСЕГДА. Пряча её по наведению, мы получили кнопку, исчезающую под своим же открытым меню (владелец 10.11: «у тебя кнопка
/// пропадает по ховеру»): курсор уходит со строки на пункт, наведение кончается, и список висит сам по себе.
pub fn button_icon(id: impl Into<ElementId>, svg: &'static [u8], tooltip: &'static str) -> KitButton {
    KitButton::new(id).ghost().xsmall().icon(Icon::default().data(svg)).tooltip(tooltip)
}

/// Пункт выпадающего списка. Действие ОТЛОЖЕНО на следующий кадр, и другого пункта конструктор не умеет.
///
/// Пункт исполняется, пока сам список ещё закрывается, и всё, что поднимает в этот миг вопрос, получает ответ «да» от того же щелчка,
/// которым пункт выбрали. Поймано живым прогоном на удалении карточки: вопрос не появлялся ни разу, а карточка исчезала. Рядом нашлись
/// ещё два таких же — и один страшнее: «Загрузить в 1С» перезаписывает конфигурацию базы, а спрашивать об этом как раз переставала.
pub fn menu_item(label: &'static str, act: impl Fn(&mut Window, &mut App) + Clone + 'static) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, window, cx| {
        let act = act.clone();
        window.defer(cx, move |window, cx| act(window, cx));
    })
}

/// Выпадающий список: ширина прибита с обеих сторон по месту его кнопки. `min_w` в одиночку только поднимает нижнюю границу, и длинные
/// слова всё равно растягивают список — тогда у каждой кнопки получается свой край.
pub fn menu(slot: Slot, menu: PopupMenu) -> PopupMenu {
    menu.min_w(slot.width()).max_w(slot.width())
}

/// Подпись поля или группы: называет ПРЕДМЕТ, а не состояние («База», не «База не указана») — закон окна, правило 4.
pub fn field_label(caption: Hsla, text: impl Into<SharedString>) -> Div {
    div().text_size(px(13.)).text_color(caption).child(text.into())
}

/// Поле-селектор карточки: подпись и ответ человека.
///
/// **При ОДНОМ варианте рисуется строкой, а не списком** (владелец 10.11, снимок «Где база» с единственным ответом «Здесь»): список, в
/// котором нечего выбрать, обещает выбор, которого нет. Когда вариантов снова станет два, селектор вернётся сам.
///
/// `items` — пары «слово ответа» и что сделать по выбору. Ширина списка — ширина поля (`Slot::Field`).
pub fn field_select(
    id: impl Into<ElementId>,
    label: &'static str,
    value: SharedString,
    caption: Hsla,
    fg: Hsla,
    items: Vec<(SharedString, Act)>,
) -> AnyElement {
    let body = if items.len() < 2 {
        // Выбора нет — и органа выбора нет: ответ стоит строкой того же вида, что значение поля.
        card(fg.opacity(0.04), caption).px_3().py_2().child(div().min_w_0().text_color(fg).child(value)).into_any_element()
    } else {
        KitButton::new(id)
            .outline()
            .w_full()
            .text_color(fg)
            .label(value)
            .dropdown_caret(true)
            .dropdown_menu(move |m, _, _| {
                let mut m = menu(Slot::Field, m);
                for (word, act) in items.iter() {
                    let act = act.clone();
                    // Слово ответа приходит из настроек карточки и живёт столько же, сколько окно: `menu_item` берёт `&'static str`,
                    // и здесь мы отдаём ему именно такую строку.
                    let word: &'static str = Box::leak(word.to_string().into_boxed_str());
                    m = m.item(menu_item(word, move |window, cx| act(window, cx)));
                }
                m
            })
            .into_any_element()
    };
    v_flex().gap_1().child(field_label(caption, label)).child(body).into_any_element()
}

/// Строка отказа в карточке: почему правка не записалась и что сделать. Размер — размер подписи поля, цвет — `danger` темы.
///
/// **Второй и последний вид слов в карточке** (закон окна, раздел «Отказ правки: слова у карточки»), и разведён с первым намертво:
/// подпись называет ПРЕДМЕТ и стоит всегда (`field_label`, правило 4), эта строка называет СОБЫТИЕ — правка не записана — и стоит
/// только пока есть что исправлять. Отдельным примитивом, а не `div()` по месту: иначе третий такой же вид слов завёлся бы в
/// соседней карточке своим размером и своим цветом — порода ляпов, ради которой конструктор и собран.
///
/// Своих слов не формулирует и формулировать не вправе: ей отдают те же, что ушли в журнал (`gui::save::breaks_connection` —
/// один отказ, одно начертание на оба места).
pub fn note_refusal(danger: Hsla, text: impl Into<SharedString>) -> Div {
    div().text_size(px(13.)).text_color(danger).child(text.into())
}

/// Ряд с полем и значком справа — вторая и последняя законная форма кнопки в карточке: выбор папки, копирование.
pub fn field_icon(caption: Hsla, label: impl Into<SharedString>, input: impl IntoElement, button: KitButton) -> Div {
    v_flex()
        .gap_1()
        .child(field_label(caption, label.into()))
        .child(h_flex().gap_2().items_center().child(div().flex_1().min_w_0().child(input)).child(button))
}

#[cfg(test)]
mod tests {
    // Точечный ввоз, а не `use super::*`: вместе со звёздочкой приезжает `gpui::test`, и обычный `#[test]` начинает раскрываться сам в
    // себя — сборка падает «recursion limit reached».
    use super::{Slot, FIELD_W, LIST_W};
    use gpui_kit::px;

    /// Ширины считаются от устройства окна, а не подбираются: список шире поля ровно на отступы карточки с её рамкой.
    #[test]
    fn the_two_widths_differ_by_the_card_padding() {
        assert_eq!(LIST_W - FIELD_W, px(26.), "отступ карточки по 12 с каждой стороны плюс рамка");
        assert_eq!(Slot::List.width(), LIST_W);
        assert_eq!(Slot::Field.width(), FIELD_W);
    }
}
