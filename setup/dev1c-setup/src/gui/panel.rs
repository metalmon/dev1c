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
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Disableable, FocusableExt, IndexPath, Sizable, Theme, TitleBar};
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

/// Варианты списков: значение в `dev1c.toml` и подпись в окне.
const ENGINES: [(&str, &str); 2] = [("docker", "docker"), ("podman", "podman")];
const SERVES: [(&str, &str); 3] = [
    ("host", "Служба на этой машине (видеокарта, nssm)"),
    ("docker", "Контейнер рядом со средой"),
    ("remote", "Удалённый сервер"),
];

/// Поля вкладки «Параметры». Числа — тот же ввод, что текст: `validate`
/// скажет словами, если вписали не то.
pub struct Fields {
    pub name: Entity<InputState>,
    pub port: Entity<InputState>,
    pub memory: Entity<InputState>,
    pub engine: Entity<SelectState<SearchableVec<SharedString>>>,
    pub gpu: bool,
    pub serve: Entity<SelectState<SearchableVec<SharedString>>>,
    pub file: Entity<InputState>,
    pub url: Entity<InputState>,
    pub key: Entity<InputState>,
    pub model_name: Entity<InputState>,
    pub model_port: Entity<InputState>,
    pub agents: Entity<InputState>,
    pub fit_ctx: Entity<InputState>,
}

/// Поле ввода с подсказкой и начальным значением.
fn field(ph: &str, val: &str, masked: bool, window: &mut Window, cx: &mut Context<Panel>) -> Entity<InputState> {
    let (ph, val) = (ph.to_string(), val.to_string());
    cx.new(|cx| {
        let s = InputState::new(window, cx).placeholder(ph).default_value(val);
        if masked {
            s.masked(true)
        } else {
            s
        }
    })
}

/// Список выбора: подписи людям, значения — в файл.
fn select(
    options: &[(&str, &str)],
    current: &str,
    window: &mut Window,
    cx: &mut Context<Panel>,
) -> Entity<SelectState<SearchableVec<SharedString>>> {
    let labels: Vec<SharedString> = options.iter().map(|(_, label)| SharedString::from(label.to_string())).collect();
    let at = options.iter().position(|(value, _)| *value == current).unwrap_or(0);
    cx.new(|cx| SelectState::new(SearchableVec::new(labels), Some(IndexPath::new(at)), window, cx))
}

/// Что выбрано в списке — значением для файла, не подписью.
fn selected<'a>(state: &Entity<SelectState<SearchableVec<SharedString>>>, options: &'a [(&str, &str)], cx: &App) -> &'a str {
    let at = state.read(cx).selected_index(cx).map(|i| i.row).unwrap_or(0);
    options.get(at).map(|(value, _)| *value).unwrap_or(options[0].0)
}

pub struct Panel {
    pub focus: FocusHandle,
    pub fields: Fields,
    /// Параметры, какими они лежат в файле: с ними сравниваем поля.
    saved: crate::cfg::Cfg,
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
    /// Обмер для подбора высоты окна по вкладке «Параметры»: видимая область
    /// тела вкладки и высота её содержимого.
    viewport_h: Option<Pixels>,
    params_h: Option<Pixels>,
    last_fit: Option<(Pixels, Pixels)>,
    fit_queued: bool,
    /// Высота, которую подгонка поставила сама. Окно стало другим — значит
    /// размер выбрал человек, и больше мы в высоту не лезем.
    fitted_h: Option<Pixels>,
}

impl Panel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Основа сравнения нормализована так же, как поля: иначе честный
        // файл с полем другой формы открывал бы окно «изменённым».
        let saved = crate::ui::for_form(cx.global::<Ui>().cfg.clone());
        let m = &saved.model;
        let fields = Fields {
            name: field("dev1c", &saved.server.name, false, window, cx),
            port: field("3390", &saved.server.port.to_string(), false, window, cx),
            memory: field("24", &crate::ui::memory_field(&saved.server.memory), false, window, cx),
            engine: select(&ENGINES, &saved.server.engine, window, cx),
            gpu: saved.server.gpu,
            serve: select(&SERVES, &m.serve, window, cx),
            file: field("C:\\models\\model.gguf", &m.file, false, window, cx),
            url: field("https://host/v1", &m.url, false, window, cx),
            key: field("ключ доступа", &m.key, true, window, cx),
            model_name: field("как сервер называет модель", &m.name, false, window, cx),
            model_port: field("1235", &m.port.to_string(), false, window, cx),
            agents: field("1", &m.agents.to_string(), false, window, cx),
            fit_ctx: field("65536", &m.fit_ctx.to_string(), false, window, cx),
        };
        let confirm = cx.new(|cx| InputState::new(window, cx).placeholder("имя контейнера для подтверждения"));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Panel {
            focus,
            fields,
            saved,
            log_scroll: ScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            log_len: 0,
            confirm,
            confirming: false,
            viewport_h: None,
            params_h: None,
            last_fit: None,
            fit_queued: false,
            fitted_h: None,
        }
    }

    /// Два обмера за кадр (видимая область и содержимое) — один подбор:
    /// отложенное замыкание читает пару, какой она стала к концу кадра.
    fn measured(&mut self, viewport: Option<Pixels>, params: Option<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        // Меряем только «Параметры»: это самая высокая вкладка, и по ней
        // одной окно выбирает высоту. Иначе плашка, появившаяся во время
        // установки, двигала бы окно, пока человек смотрит журнал.
        if cx.global::<Ui>().tab != Tab::Params {
            return;
        }
        // Человек сам изменил высоту — больше не спорим.
        if !crate::ui::should_fit(self.fitted_h.map(f32::from), f32::from(window.bounds().size.height)) {
            return;
        }
        self.viewport_h = viewport.or(self.viewport_h);
        self.params_h = params.or(self.params_h);
        let (Some(v), Some(c)) = (self.viewport_h, self.params_h) else { return };
        if self.fit_queued || !crate::gui::window::needs_fit(self.last_fit, (v, c)) {
            return;
        }
        self.fit_queued = true;
        // Два обмера за кадр (видимая область и содержимое) — один подбор:
        // отложенное замыкание читает пару, какой она стала к концу кадра.
        cx.defer_in(window, |this, window, cx| {
            this.fit_queued = false;
            let (Some(v), Some(c)) = (this.viewport_h, this.params_h) else { return };
            if !crate::gui::window::needs_fit(this.last_fit, (v, c)) {
                return;
            }
            this.last_fit = Some((v, c));
            if let Some(h) = crate::gui::window::fit_height(window, cx, c, v) {
                this.fitted_h = Some(h);
            }
        });
    }

    /// Параметры, как они набраны в полях. Числа, которые не разобрались,
    /// остаются прежними: пустое поле не должно обнулять порт молча — об
    /// ошибке скажет `validate` после «Сохранить».
    fn edited(&self, cx: &App) -> crate::cfg::Cfg {
        let v = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let mut cfg = self.saved.clone();
        cfg.server.name = v(&self.fields.name);
        cfg.server.port = v(&self.fields.port).trim().parse().unwrap_or(cfg.server.port);
        cfg.server.memory = crate::ui::memory_value(&v(&self.fields.memory));
        cfg.server.engine = selected(&self.fields.engine, &ENGINES, cx).to_string();
        cfg.server.gpu = self.fields.gpu;
        cfg.model.serve = selected(&self.fields.serve, &SERVES, cx).to_string();
        cfg.model.file = v(&self.fields.file);
        cfg.model.url = v(&self.fields.url);
        cfg.model.key = v(&self.fields.key);
        cfg.model.name = v(&self.fields.model_name);
        cfg.model.port = v(&self.fields.model_port).trim().parse().unwrap_or(cfg.model.port);
        cfg.model.agents = v(&self.fields.agents).trim().parse().unwrap_or(cfg.model.agents);
        cfg.model.fit_ctx = v(&self.fields.fit_ctx).trim().parse().unwrap_or(cfg.model.fit_ctx);
        crate::ui::for_form(cfg)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        crate::ui::fields_differ(&self.saved, &self.edited(cx))
    }

    /// Записать параметры и сказать об этом в подвале. Ошибка — строкой,
    /// файл не тронут, набранное остаётся в полях.
    fn save(&mut self, cx: &mut Context<Self>) {
        let cfg = self.edited(cx);
        let kit = cx.global::<Ui>().kit.clone();
        let r = crate::ui::save_to(&kit, &cfg);
        let line = match &r {
            Ok(()) => Line::ok(format!("Сохранено в {}", crate::cfg::FILE)),
            Err(e) => Line::fail(e.clone()),
        };
        {
            let ui = cx.global_mut::<Ui>();
            ui.status = Some(line.clone());
            gui::note_line(ui, line);
        }
        if r.is_ok() {
            self.saved = cfg.clone();
            let ui = cx.global_mut::<Ui>();
            ui.cfg = cfg;
            // Параметры приняты — файл больше не «не прочитан».
            ui.cfg_error = None;
            // Состояние операций считается от параметров: оно уже другое.
            gui::refresh_ops(cx);
        }
        cx.refresh_windows();
    }

    /// Выбор файла модели: на время диалога окно не прячется (`dialog_open`),
    /// после — путь ложится в поле, а в файл его отнесёт «Сохранить».
    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let target = self.fields.file.clone();
        cx.global_mut::<Ui>().dialog_open = true;
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |_, cx| {
            let picked = rx.await.ok().and_then(|r| r.ok()).flatten().and_then(|v| v.into_iter().next());
            let _ = cx.update(|cx| {
                cx.global_mut::<Ui>().dialog_open = false;
                let ui = cx.global::<Ui>();
                let (Some(w), Some(panel)) = (ui.window, ui.panel.clone()) else { return };
                let _ = w.update(cx, |_, window, cx| {
                    window.activate_window();
                    if let Some(p) = &picked {
                        let path = p.display().to_string();
                        // Фильтра по расширению у системного запроса нет —
                        // говорим словами, а не берём молча.
                        if let Some(warn) = crate::ui::gguf_warning(&path) {
                            gui::note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: warn });
                        }
                        target.update(cx, |s, cx| s.set_value(path, window, cx));
                    }
                    panel.update(cx, |this, cx| this.focus.focus(window, cx));
                });
            });
        })
        .detach();
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

    /// Поле по Fluent: без кольца фокуса, у активного — акцентная линия 2 px
    /// снизу (так же в `1c-connect` и так было в прежнем `app.css`).
    fn input(state: &Entity<InputState>, accent: Hsla, stroke: Hsla, window: &Window, cx: &App) -> impl IntoElement {
        let focused = state.read(cx).focus_handle(cx).is_focused(window);
        let (h, colour) = if focused { (px(2.), accent) } else { (px(1.), stroke) };
        div()
            .relative()
            .w_full()
            .child(Input::new(state).focus_bordered(false))
            // Нижняя линия поля — подпись Fluent: в покое сильнее рамки,
            // у активного поля акцентная и вдвое толще.
            .child(div().absolute().bottom_0().left_1().right_1().h(h).rounded_b_sm().bg(colour))
    }

    /// Строка списка по прежнему окну: отступы 10/8, разделитель между
    /// строками и подсветка под курсором.
    fn list_row(id: &'static str, ix: usize, row_border: Hsla, hover: Hsla) -> Stateful<Div> {
        div()
            .id((id, ix))
            .flex()
            .flex_row()
            .items_start()
            .gap_3()
            .px_2()
            .py(px(10.))
            .rounded(px(4.))
            .when(ix > 0, |d| d.border_t_1().border_color(row_border))
            .hover(|s| s.bg(hover))
    }

    /// Содержимое вкладки белой карточкой: на сером поле панели карточка
    /// читается как карточка, а не как лист без краёв.
    fn body_card(layer: Hsla, border: Hsla, content: impl IntoElement) -> Div {
        Self::layer(layer, border).flex().flex_col().size_full().p_4().child(content)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        // Слои Fluent, как в прежнем окне: серое основание, белые слои
        // поверх, карточка состояния — слой 1С-палитры.
        let (ground, layer, border, fg, accent) = (t.muted, t.background, t.border, t.foreground, t.primary);
        let (success, warning, danger, card) = (t.success, t.warning, t.danger, t.secondary);
        let (stroke, row_border) = (t.input, t.table_row_border);
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
        let notice = crate::ui::cfg_notice(ui.kit.path(crate::cfg::FILE).exists(), ui.cfg_error.as_deref())
            .or_else(|| crate::ui::notice(&ops));
        // Несохранённые правки и непрочитанный файл главную кнопку не
        // пускают: установка идёт по файлу, и молча применить не то — обман.
        let dirty = self.is_dirty(cx);
        let cfg_unread = ui.cfg_error.is_some();
        let blocked_why = crate::ui::primary_blocked(dirty, cfg_unread);
        let foot_left = match (blocked_why, &ui.status) {
            // Почему главная кнопка не работает — текстом в подвале: всплывающая
            // подсказка на погасшей кнопке молчит, пока на неё не наведут.
            (Some(why), _) => (why.to_string(), true),
            (None, Some(l)) => (l.text.clone(), l.kind == Kind::Fail),
            (None, None) => (ui.kit.root.display().to_string(), false),
        };
        let working = ui.working();
        let busy_now = busy.is_some();
        let todo = ops.iter().filter(|o| o.dot == Dot::Off || o.dot == Dot::Busy).count();

        // Заголовок окна: значок, название, кнопки Windows рисует TitleBar.
        // Фон — фон окна: своя полоса акцентом спорила бы с единственной
        // акцентной кнопкой в шапке.
        let title_bar = TitleBar::new().bg(layer).border_b_1().border_color(border.opacity(0.6)).child(
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
                .disabled(busy_now || blocked_why.is_some())
                .when_some(blocked_why, |b, why| b.tooltip(why))
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
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(card_now.state.clone()))
                    .when(!card_now.sub.is_empty(), |v| v.child(div().text_size(px(12.)).text_color(caption).child(card_now.sub.clone()))),
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
            (n, true) => KitTab::new().label(Tab::Ops.title()).suffix(badge(n, danger, layer)),
            (n, false) => KitTab::new().label(Tab::Ops.title()).suffix(badge(n, border, fg)),
        };
        let check_tab = match failed {
            0 => KitTab::new().label(Tab::Check.title()),
            n => KitTab::new().label(Tab::Check.title()).suffix(badge(n, danger, layer)),
        };
        let log_tab = match unread {
            n if n > 0 && tab != Tab::Log => KitTab::new().label(Tab::Log.title()).suffix(badge(n, border, fg)),
            _ => KitTab::new().label(Tab::Log.title()),
        };
        let tabs = TabBar::new("tabs")
            .underline()
            .small()
            .selected_index(Tab::ALL.iter().position(|x| *x == tab).unwrap_or(0))
            // Порядок — `Tab::ALL`, он же порядок индексов щелчка.
            .children([KitTab::new().label(Tab::Params.title()), ops_tab, KitTab::new().label(Tab::Access.title()), check_tab, log_tab])
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
                Self::body_card(layer, border, Self::scrolled(
                    "ops",
                    &self.body_scroll,
                    v_flex().children(rows.into_iter().enumerate().map(|(ix, o)| {
                        Self::list_row("ops", ix, row_border, ground)
                            .child(div().size_2p5().mt(px(6.)).flex_shrink_0().rounded_full().bg(Self::dot(o.dot, &theme)))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(div().child(o.title))
                                    .child(div().text_size(px(12.)).text_color(caption).child(o.detail)),
                            )
                    })),
                ))
                .into_any_element()
            }
            Tab::Log => {
                let lines = cx.global::<Ui>().log.clone();
                if lines.len() != self.log_len {
                    self.log_len = lines.len();
                    self.log_scroll.scroll_to_bottom();
                }
                Self::body_card(layer, border, Self::scrolled(
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
                                .child(div().text_size(px(12.)).text_color(caption).child(time))
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
                ))
                .into_any_element()
            }
            Tab::Access => {
                let ui = cx.global::<Ui>();
                let host = ui.addresses.first().cloned().unwrap_or_else(|| crate::ui::ADDRESS_UNKNOWN.to_string());
                let others = ui.addresses.iter().skip(1).cloned().collect::<Vec<_>>();
                let rows = crate::ui::access_rows(&ui.cfg, ui.facts, &host);
                Self::body_card(layer, border, Self::scrolled(
                    "access",
                    &self.body_scroll,
                    v_flex()
                        .when(!others.is_empty(), |v| {
                            v.child(
                                div()
                                    .pb_2()
                                    .text_size(px(12.))
                                    .text_color(caption)
                                    .child(format!("другие адреса этой машины: {}", others.join(", "))),
                            )
                        })
                        .children(rows.into_iter().enumerate().map(|(ix, r)| {
                        Self::list_row("access", ix, row_border, ground)
                            .child(div().w(px(150.)).flex_shrink_0().text_color(caption).child(r.what))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    // Значение — моноширинным: это то, что
                                    // человек перепечатывает или копирует.
                                    .child(div().font_family("ui-monospace").child(r.value))
                                    .child(div().text_size(px(12.)).text_color(caption).child(r.detail)),
                            )
                    })),
                ))
                .into_any_element()
            }
            Tab::Check => {
                let checks = cx.global::<Ui>().checks.clone();
                if checks.is_empty() {
                    Self::body_card(layer, border, div().text_color(caption).child("Проверки ещё не было — кнопка «Проверить» в шапке."))
                        .into_any_element()
                } else {
                    let (passed, failed) = common::check::summary(&checks);
                    Self::body_card(layer, border, Self::scrolled(
                        "check",
                        &self.body_scroll,
                        v_flex()
                            .gap_2()
                            .py_1()
                            .child(
                                div()
                                    .text_color(if failed == 0 { success } else { danger })
                                    .child(format!("прошло {passed}, провалено {failed}")),
                            )
                            .children(checks.into_iter().enumerate().map(|(ix, c)| {
                                let dot = match (c.ok, c.info) {
                                    (true, false) => Dot::Up,
                                    (true, true) => Dot::Off,
                                    _ => Dot::Down,
                                };
                                Self::list_row("check", ix, row_border, ground)
                                    .child(div().size_2p5().mt(px(6.)).flex_shrink_0().rounded_full().bg(Self::dot(dot, &theme)))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(div().child(format!("{}: {}", c.what, c.detail)))
                                            .when(!c.ok && !c.hint.is_empty(), |v| {
                                                v.child(div().text_size(px(12.)).text_color(warning).child(c.hint.clone()))
                                            }),
                                    )
                            })),
                    ))
                    .into_any_element()
                }
            }
            // Обмер содержимого «Параметров»: по нему окно подбирает высоту.
            Tab::Params => {
                let f = &self.fields;
                let serve = selected(&f.serve, &SERVES, cx);
                let local = serve != "remote";
                // Подпись Caption приглушённым, поле, подсказка — как в
                // прежнем окне и как в `1c-connect`.
                let labelled = |label: &'static str, body: AnyElement| {
                    v_flex().w_full().gap_1().child(div().child(label)).child(body)
                };
                let input = |label: &'static str, state: &Entity<InputState>| {
                    labelled(label, Self::input(state, accent, stroke, window, cx).into_any_element())
                };
                let group = |title: &'static str| {
                    Self::layer(layer, border)
                        .flex()
                        .flex_col()
                        .w_full()
                        .p_4()
                        .gap_4()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                };
                Self::scrolled(
                    "params",
                    &self.body_scroll,
                    div()
                        // Высота содержимого вкладки: по ней окно подбирает свою.
                        .on_children_prepainted({
                            let m = cx.listener(|this: &mut Self, b: &Vec<Bounds<Pixels>>, window, cx| {
                                this.measured(None, b.first().map(|b| b.size.height), window, cx)
                            });
                            move |b, window, cx| m(&b, window, cx)
                        })
                        .child(
                    // Две группы рядом, каждая своей карточкой: границы
                    // карточек делят колонки и показывают, где группа
                    // кончается (решение заказчика 2026-10-19).
                    h_flex()
                        .items_start()
                        .gap_4()
                        .py_1()
                        .child(
                            group("Контейнер")
                                .flex_1()
                                .min_w_0()
                                .child(input("Имя контейнера и приставка его томов", &f.name))
                                .child(input("Первый порт: RDP, дальше Gitea, модель, SSH", &f.port))
                                .child(input("Предел памяти контейнера, ГБ", &f.memory))
                                .child(labelled("Движок контейнеров", div().w_full().child(Select::new(&f.engine)).into_any_element()))
                                .child(
                                    h_flex().items_center().gap_2().child(Switch::new("gpu").focus_ring(false).checked(f.gpu).on_change(cx.listener(
                                        |panel, checked: &bool, _, cx| {
                                            panel.fields.gpu = *checked;
                                            cx.refresh_windows();
                                        },
                                    )))
                                    .child(div().child("Отдать контейнеру GPU")),
                                ),
                        )
                        .child(
                            group("Модель для агентов")
                                .flex_1()
                                .min_w_0()
                                .child(labelled("Где работает сервер модели", div().w_full().child(Select::new(&f.serve)).into_any_element()))
                                .when(local, |g| {
                                    g.child(labelled(
                                        "Файл модели (.gguf)",
                                        h_flex()
                                            .gap_2()
                                            .child(div().flex_1().min_w_0().child(Self::input(&f.file, accent, stroke, window, cx)))
                                            .child(Button::new("pick-file").label("Выбрать…").on_click(cx.listener(
                                                |panel: &mut Self, _, _, cx| panel.pick_file(cx),
                                            )))
                                            .into_any_element(),
                                    ))
                                })
                                .when(!local, |g| {
                                    g.child(input("Адрес сервера, без завершающего /chat/completions", &f.url))
                                        .child(input("Ключ доступа", &f.key))
                                })
                                .child(input(if local { "Имя модели; пусто — имя файла" } else { "Имя модели, как его называет сервер" }, &f.model_name))
                                .when(serve == "host", |g| g.child(input("Порт службы модели на этой машине", &f.model_port)))
                                // Делёж окна и его нижняя граница — про свой
                                // сервер: у чужого ни того, ни другого не
                                // спрашивают (так было и в прежнем окне).
                                .when(local, |g| {
                                    g.child(input("Делить окно на агентов; 1 — не делить", &f.agents))
                                        .child(input("Нижняя граница окна, токенов", &f.fit_ctx))
                                }),
                        ),
                        ),
                )
                .into_any_element()
            }
        };

        // Несохранённое и непрочитанное правится на «Параметрах»: пока это
        // так, в подвале одна кнопка — туда. Чистку и установку в это время
        // всё равно нельзя пускать по файлу, который разошёлся с полями.
        let to_params: Option<AnyElement> = ((dirty || cfg_unread) && tab != Tab::Params).then(|| {
            Button::new("to-params")
                .primary()
                .label("К параметрам")
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.global_mut::<Ui>().tab = Tab::Params;
                    cx.refresh_windows();
                }))
                .into_any_element()
        });
        let foot_actions: AnyElement = match tab {
            _ if to_params.is_some() => to_params.unwrap(),
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
                        row.child(div().w(px(260.)).child(Self::input(&confirm, accent, stroke, window, cx)))
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
            Tab::Params => Button::new("save")
                .primary()
                .label("Сохранить")
                .disabled(!dirty || busy_now)
                .on_click(cx.listener(|panel: &mut Self, _, _, cx| panel.save(cx)))
                .into_any_element(),
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

        // Раскладка слоями, как в прежнем окне: белая шапка с карточкой
        // состояния, серое основание, белая панель вкладки со своим подвалом.
        let head = v_flex()
            .bg(layer)
            .border_b_1()
            .border_color(border)
            .px_6()
            .pt_4()
            .pb(px(14.))
            .gap_3()
            .child(status);

        let progress = if working {
                // Работа: дорожка акцентом темы, бегунок по кругу.
                div().w_full().h(px(2.)).relative().bg(border.opacity(0.6)).child(
                    div().absolute().top_0().h(px(2.)).w(relative(0.25)).rounded_full().bg(accent).with_animation(
                        "progress-thumb",
                        Animation::new(Duration::from_millis(1200)).repeat(),
                        |d, t| d.left(relative(-0.25 + 1.25 * t)),
                    ),
                )
            } else {
                // Покой: линия 1 px в полосе той же высоты — подвал не прыгает.
            div().w_full().h(px(2.)).child(div().w_full().h(px(1.)).mt(px(1.)).bg(border.opacity(0.6)))
        };

        // Подвал одной высоты на всех вкладках: кнопки есть не везде, а
        // итог последнего действия бывает длинным — высота от этого прыгать
        // не должна, поэтому высота задана, а текст укладывается в строку.
        let footer = h_flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .h(px(56.))
            .bg(layer)
            .px_6()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_ellipsis()
                    .text_color(if foot_left.1 { danger } else { caption })
                    .child(foot_left.0),
            )
            .child(foot_actions);

        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .bg(ground)
            .text_color(fg)
            .text_size(px(14.))
            .child(title_bar)
            .child(head)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    // Сетка отступов прежнего окна: 24 по бокам, 12 сверху.
                    .px_6()
                    .pt_3()
                    .pb_3()
                    .gap_3()
                    .children(notice.map(|text| {
                        Self::layer(warning.opacity(0.15), warning)
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap_2()
                            .px_3()
                            .py_2p5()
                            .child(div().w(px(16.)).flex_shrink_0().text_color(warning).child("!"))
                            .child(div().flex_1().min_w_0().child(text))
                    }))
                    .child(div().border_b_1().border_color(border).child(tabs))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .on_children_prepainted({
                                let m = cx.listener(|this: &mut Self, b: &Vec<Bounds<Pixels>>, window, cx| {
                                    this.measured(b.first().map(|b| b.size.height), None, window, cx)
                                });
                                move |b, window, cx| m(&b, window, cx)
                            })
                            .child(body),
                    ),
            )
            .child(progress)
            .child(footer)
    }
}
