//! Содержимое окна: свой заголовок Windows, карточка состояния, плашка
//! предупреждения, пять вкладок (Операции, Параметры, Доступ, Проверка,
//! Журнал), тело вкладки, полоса хода в разделителе и подвал.
//!
//! Что говорит карточка и плашка, решает `crate::ui` — там это с тестами.
//! Здесь только вид: `render` читает глобал `Ui`. Изменили `Ui` —
//! `cx.refresh_windows()`.

use crate::gui::{self, Ui};
use crate::ui::{Busy, Dot, Kind, Line, Tab, Which};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Disableable, Sizable, Theme, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::Duration;

/// Значок программы в заголовке — тот же PNG, что у окна.
const ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");

fn icon() -> Arc<Image> {
    static I: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    I.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, ICON_PNG.to_vec()))).clone()
}

pub struct Panel {
    pub focus: FocusHandle,
    /// Полоса прокрутки тела вкладки: своя у журнала, своя у остального.
    log_scroll: ScrollHandle,
    body_scroll: ScrollHandle,
    /// Сколько строк было в журнале на прошлой отрисовке — по изменению
    /// прокручиваем журнал к последней строке.
    log_len: usize,
    /// Имя контейнера, набранное для подтверждения удаления данных.
    confirm: Entity<InputState>,
    /// Показано ли поле подтверждения.
    confirming: bool,
}

impl Panel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let confirm = cx.new(|cx| InputState::new(window, cx).placeholder("имя контейнера для подтверждения"));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Panel { focus, log_scroll: ScrollHandle::new(), body_scroll: ScrollHandle::new(), log_len: 0, confirm, confirming: false }
    }

    /// Слой Fluent: та же плашка, что в `1c-connect` — фон, рамка, скругление.
    fn layer(card: Hsla, border: Hsla) -> Div {
        div().rounded_lg().bg(card).border_1().border_color(border.opacity(0.6))
    }

    fn dot(d: Dot, t: &Theme) -> Hsla {
        match d {
            Dot::Off => t.muted_foreground,
            Dot::Busy => t.primary,
            Dot::Up => t.success,
            Dot::Down => t.danger,
        }
    }

    /// Прокручиваемая область с полосой поверх текста.
    fn scrolled(id: &'static str, handle: &ScrollHandle, content: impl IntoElement) -> impl IntoElement {
        div()
            .id(id)
            .size_full()
            .relative()
            .child(div().id(format!("{id}-area")).size_full().overflow_y_scroll().track_scroll(handle).pr_2().child(content))
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(handle).id(format!("{id}-scrollbar")).viewport_from_layout(),
                ),
            )
    }
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let (bg, border, fg, accent) = (t.background, t.border, t.foreground, t.primary);
        let (success, warning, danger, card) = (t.success, t.warning, t.danger, t.secondary);
        let theme = t.clone();
        // Подписи и подробности — основной текст вполсилы: `muted_foreground`
        // отдан подсказкам полей, для настоящего текста он недобирает контраста.
        let caption = fg.opacity(0.75);

        let ui = cx.global::<Ui>();
        let (tab, busy) = (ui.tab, ui.busy);
        let (unread, failed) = (ui.unread, ui.failed_checks);
        let ops = ui.ops.clone();
        let name = ui.cfg.server.name.clone();
        let card_now = crate::ui::status_line(&ops, &name);
        // Плашка: сначала непрочитанные параметры, потом заблокированная операция.
        let notice = ui
            .cfg_error
            .as_ref()
            .map(|e| format!("Параметры не прочитаны: {e}. Показаны значения по умолчанию, «Сохранить» перезапишет файл."))
            .or_else(|| crate::ui::notice(&ops));
        let foot_left = match &ui.status {
            Some(l) => (l.text.clone(), l.kind == Kind::Fail),
            None => (ui.kit.root.display().to_string(), false),
        };
        let working = ui.working();
        let busy_now = busy.is_some();
        let todo = ops.iter().filter(|o| o.dot == Dot::Off || o.dot == Dot::Busy).count();

        // Заголовок окна: значок, название, кнопки Windows рисует TitleBar.
        let title_bar = TitleBar::new().child(
            h_flex()
                .items_center()
                .gap_2()
                .child(img(icon()).size_4())
                .child(div().font_weight(FontWeight::SEMIBOLD).child("dev1c-setup — среда разработки 1С")),
        );

        // Карточка состояния: кружок, состояние, подробность, две кнопки.
        let primary_btn = card_now.primary.map(|(which, label)| {
            Button::new("primary")
                .primary()
                .label(label)
                .disabled(busy_now)
                .loading(busy == Some(Busy::Run))
                .on_click(move |_, _, cx| gui::run_op(cx, which))
        });
        let status = Self::layer(card, border)
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .p_3()
            .child(div().size_3().flex_shrink_0().rounded_full().bg(Self::dot(card_now.dot, &theme)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(card_now.state.clone()))
                    .when(!card_now.sub.is_empty(), |v| v.child(div().text_size(px(13.)).text_color(caption).child(card_now.sub.clone()))),
            )
            .child(
                Button::new("check")
                    .label("Проверить")
                    .disabled(busy_now)
                    .loading(busy == Some(Busy::Check))
                    .on_click(|_, _, cx| gui::run_check(cx)),
            )
            .children(primary_btn);

        // Бейджи вкладок: несделанное у «Операций» (красным, если разошлось),
        // провалы у «Проверки», непрочитанные у «Журнала».
        let badge = |n: usize, colour: Hsla, on: Hsla| div().rounded_sm().px_1().bg(colour).text_color(on).child(n.to_string());
        let ops_tab = match (todo, ops.iter().any(|o| o.dot == Dot::Busy)) {
            (0, _) => KitTab::new().label(Tab::Ops.title()),
            (n, true) => KitTab::new().label(Tab::Ops.title()).suffix(badge(n, danger, bg)),
            (n, false) => KitTab::new().label(Tab::Ops.title()).suffix(badge(n, border, fg)),
        };
        let check_tab = match failed {
            0 => KitTab::new().label(Tab::Check.title()),
            n => KitTab::new().label(Tab::Check.title()).suffix(badge(n, danger, bg)),
        };
        let log_tab = match unread {
            n if n > 0 && tab != Tab::Log => KitTab::new().label(Tab::Log.title()).suffix(badge(n, border, fg)),
            _ => KitTab::new().label(Tab::Log.title()),
        };
        let tabs = TabBar::new("tabs")
            .underline()
            .small()
            .selected_index(Tab::ALL.iter().position(|x| *x == tab).unwrap_or(0))
            .children([ops_tab, KitTab::new().label(Tab::Params.title()), KitTab::new().label(Tab::Access.title()), check_tab, log_tab])
            .on_click(cx.listener(|_, i: &usize, _, cx| {
                let Some(t) = Tab::ALL.get(*i).copied() else { return };
                let ui = cx.global_mut::<Ui>();
                ui.tab = t;
                if t == Tab::Log {
                    ui.unread = 0;
                }
                cx.refresh_windows();
            }));

        let body: AnyElement = match tab {
            Tab::Ops => {
                let rows = ops.clone();
                Self::scrolled(
                    "ops",
                    &self.body_scroll,
                    v_flex().gap_2().py_1().children(rows.into_iter().map(|o| {
                        h_flex()
                            .items_start()
                            .gap_3()
                            .child(div().size_2p5().mt(px(6.)).flex_shrink_0().rounded_full().bg(Self::dot(o.dot, &theme)))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(div().child(o.title))
                                    .child(div().text_size(px(13.)).text_color(caption).child(o.detail)),
                            )
                    })),
                )
                .into_any_element()
            }
            Tab::Log => {
                let lines = cx.global::<Ui>().log.clone();
                if lines.len() != self.log_len {
                    self.log_len = lines.len();
                    self.log_scroll.scroll_to_bottom();
                }
                Self::scrolled(
                    "log",
                    &self.log_scroll,
                    v_flex().gap_1().text_size(px(14.)).children(lines.into_iter().map(|l| {
                        if l.kind == Kind::Head {
                            // Заголовок раздела: операция слева полужирным,
                            // время справа мелким приглушённым.
                            let (title, time) = l.text.rsplit_once(" · ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((l.text.clone(), String::new()));
                            h_flex()
                                .mt_2()
                                .items_baseline()
                                .justify_between()
                                .gap_2()
                                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(fg).child(title))
                                .child(div().text_size(px(13.)).text_color(caption).child(time))
                                .into_any_element()
                        } else {
                            // Цвет — только у значка: строки, окрашенные целиком,
                            // сливаются в пёстрый лист.
                            let mark_color = match l.kind {
                                Kind::Ok => success,
                                Kind::Warn => warning,
                                Kind::Fail => danger,
                                Kind::Say | Kind::Head => caption,
                            };
                            let text_color = if l.kind == Kind::Say { caption } else { fg };
                            h_flex()
                                .items_start()
                                .child(div().w(px(18.)).flex_shrink_0().text_color(mark_color).child(l.kind.mark()))
                                .child(div().flex_1().min_w_0().text_color(text_color).child(l.text))
                                .into_any_element()
                        }
                    })),
                )
                .into_any_element()
            }
            // Параметры, Доступ и Проверка кладут задачи 5 и 6 плана.
            _ => div().text_color(caption).child("—").into_any_element(),
        };

        let foot_actions: AnyElement = match tab {
            Tab::Ops => {
                let confirming = self.confirming;
                let confirm = self.confirm.clone();
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("clean-stack")
                            .label("Снять контейнер")
                            .disabled(busy_now)
                            .on_click(|_, _, cx| gui::run_op(cx, Which::CleanStack)),
                    )
                    .when(!confirming, |row| {
                        row.child(Button::new("ask-clean-data").danger().label("Удалить данные…").disabled(busy_now).on_click(
                            cx.listener(|panel, _, _, cx| {
                                panel.confirming = true;
                                cx.refresh_windows();
                            }),
                        ))
                    })
                    .when(confirming, |row| {
                        // Удаление данных подтверждается именем контейнера:
                        // то же правило, что было в окне на Tauri.
                        row.child(div().w(px(260.)).child(Input::new(&confirm).small().focus_bordered(false)))
                            .child(Button::new("clean-data").danger().label("Удалить").disabled(busy_now).on_click(cx.listener(
                                |panel, _, _, cx| {
                                    let typed = panel.confirm.read(cx).value().to_string();
                                    let name = cx.global::<Ui>().cfg.server.name.clone();
                                    if typed == name {
                                        panel.confirming = false;
                                        gui::run_op(cx, Which::CleanData);
                                    } else {
                                        gui::note_line(cx.global_mut::<Ui>(), Line::fail(format!("для удаления данных впишите имя контейнера: {name}")));
                                    }
                                    cx.refresh_windows();
                                },
                            )))
                            .child(Button::new("cancel-clean").label("Отмена").on_click(cx.listener(|panel: &mut Self, _, _, cx| {
                                panel.confirming = false;
                                cx.refresh_windows();
                            })))
                    })
                    .into_any_element()
            }
            Tab::Log => Button::new("clear-log")
                .label("Очистить")
                .disabled(busy_now)
                .on_click(|_, _, cx| {
                    let ui = cx.global_mut::<Ui>();
                    ui.log.clear();
                    ui.unread = 0;
                    cx.refresh_windows();
                })
                .into_any_element(),
            _ => div().into_any_element(),
        };

        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_size(px(15.))
            .child(title_bar)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3()
                    .child(status)
                    .children(notice.map(|text| {
                        Self::layer(warning.opacity(0.15), warning)
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap_2()
                            .p_2()
                            .child(div().w(px(16.)).flex_shrink_0().text_color(warning).child("!"))
                            .child(div().flex_1().min_w_0().text_size(px(13.)).child(text))
                    }))
                    .child(div().border_b_1().border_color(border.opacity(0.6)).child(tabs))
                    .child(div().flex_1().min_h_0().child(body))
                    .child(if working {
                        // Работа: дорожка акцентом темы, бегунок по кругу.
                        div().w_full().h(px(2.)).relative().bg(border.opacity(0.6)).child(
                            div().absolute().top_0().h(px(2.)).w(relative(0.25)).rounded_full().bg(accent).with_animation(
                                "progress-thumb",
                                Animation::new(Duration::from_millis(1200)).repeat(),
                                |d, t| d.left(relative(-0.25 + 1.25 * t)),
                            ),
                        )
                    } else {
                        // Покой: линия 1 px в полосе той же высоты.
                        div().w_full().h(px(2.)).child(div().w_full().h(px(1.)).mt(px(1.)).bg(border.opacity(0.6)))
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .pt_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(13.))
                                    .text_color(if foot_left.1 { danger } else { caption })
                                    .child(foot_left.0),
                            )
                            .child(foot_actions),
                    ),
            )
    }
}
