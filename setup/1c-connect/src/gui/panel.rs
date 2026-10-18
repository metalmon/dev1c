//! Содержимое флайаута: шапка, карточка состояния, три вкладки-пивота
//! (Параметры, Проект, Журнал), тело вкладки и подвал — слева итог
//! последнего действия, справа одна акцентная кнопка главного действия и
//! кнопка «…» с меню остального.
//!
//! Окно пересобирается на каждый показ, поэтому `Panel` держит только
//! сущности полей, фокус и прокрутку журнала; всё остальное читается из
//! глобала `Ui` в `render`. Изменили `Ui` — `cx.refresh_windows()`.

use crate::cfg::{self, Host};
use crate::gui::theme;
use crate::gui::window::hide_window;
use crate::gui::{self, Busy, Dot, Kind, Line, ProjectView, Ui, Which};
use crate::session::State;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Disableable, FocusableExt, Icon, Sizable, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::Duration;

/// Значок программы в шапке — тот же PNG, что у окна и у трея.
const ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");

/// Значок меню «Ещё»: три линии (Lucide `menu`).
const MENU_SVG: &[u8] = include_bytes!("../../icons/menu.svg");

fn icon() -> Arc<Image> {
    static I: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    I.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, ICON_PNG.to_vec()))).clone()
}

/// Поля вкладки «Параметры». Рабочее место — обычный ввод, а не список:
/// логины среды заводят позже, допустимые значения стережёт `cfg::validate`.
/// Адреса публикации в окне нет — он считается от каталога базы
/// (`cfg::derived_base_url`).
pub struct Fields {
    pub server: Entity<InputState>,
    pub password: Entity<InputState>,
    pub base_path: Entity<InputState>,
    pub db_user: Entity<InputState>,
    pub db_password: Entity<InputState>,
    pub project: Entity<InputState>,
    pub workstation: Entity<InputState>,
}

pub struct Panel {
    pub focus: FocusHandle,
    pub fields: Fields,
    log_scroll: ScrollHandle,
    /// Полоса прокрутки «Проекта» — своя ручка, отдельная от журнала.
    project_scroll: ScrollHandle,
    /// Сколько строк журнала было на прошлой отрисовке: чтобы
    /// толкать полосу только на новые, иначе целиком вниз.
    log_len: usize,
    original: Host,
    /// Высота видимой области тела вкладки и высота содержимого «Параметров»
    /// с последней отрисовки.
    viewport_h: Option<Pixels>,
    params_h: Option<Pixels>,
    /// Пара (видимая область, содержимое), по которой подбирали высоту в
    /// прошлый раз — `None` до первого подбора. Новый обмер подбирает
    /// заново, когда отличается от неё больше чем на 1 px (`window::needs_fit`):
    /// подвал, переносящийся на вторую строку после первого подбора, меняет
    /// видимую область и без повторной подгонки окно прокручивалось бы на
    /// высоту этой строки.
    last_fit: Option<(Pixels, Pixels)>,
    /// Подбор уже отложен до конца кадра: второй обмер в том же кадре его
    /// не дублирует.
    fit_queued: bool,
}

/// Кружок, состояние и подстрока карточки. Порты — аргументом, чтобы
/// функция оставалась чистой.
pub fn status_line(l: &State, ports: &cfg::Ports) -> (Dot, &'static str, String) {
    match l {
        State::Off => (Dot::Off, "Не подключён", "туннель в среду разработки выключен".into()),
        State::Connecting => (Dot::Busy, "Подключаюсь…", String::new()),
        State::Up { since } => {
            let m = since.elapsed().as_secs() / 60;
            (Dot::Up, "Подключён", format!("{m} мин · mcp-1c {} · web {}", ports.mcp, ports.web))
        }
        State::Retrying(why) => (Dot::Busy, "Переподключаюсь", why.clone()),
        State::Failed(why) => (Dot::Down, "Не подключён", why.clone()),
    }
}

/// Подсказка под каталогом проекта, когда раскладки ещё нет.
pub const HINT_PROJECT: &str = "Параметры → Проект, затем «Установить»";

/// Вкладка «Проект» без ответа от `refresh_project`.
const PROJECT_EMPTY: &str = "Каталог проекта задаётся в «Параметрах»; install раскладывает git.";

/// Строки вкладки «Проект»: кружок, заголовок, подробность. Чистая — вид
/// только раскладывает её по слоям. Кружка нет (`None`) у подсказки: она
/// не факт о проекте, а объяснение, и рисуется мелким текстом.
pub fn project_rows(v: Option<&ProjectView>) -> Vec<(Option<Dot>, String, String)> {
    let Some(v) = v else { return vec![(None, PROJECT_EMPTY.into(), String::new())] };
    if !v.ok {
        return vec![(Some(Dot::Busy), v.dir.clone(), v.why.clone()), (None, HINT_PROJECT.into(), String::new())];
    }
    let mut rows = vec![(Some(Dot::Up), v.dir.clone(), format!("ветка {}", v.branch))];
    rows.extend(v.lines.iter().map(|l| match l.split_once(": ") {
        Some((title, detail)) => (Some(Dot::Off), title.to_string(), detail.to_string()),
        None => (Some(Dot::Off), l.clone(), String::new()),
    }));
    rows
}

/// Параметры из полей. `dump`, `tools` и `base_url` пустые — их подставит
/// `save_host` из текущих, в окне этих полей нет.
pub fn host_of(f: &Fields, cx: &App) -> Host {
    let v = |e: &Entity<InputState>| e.read(cx).value().to_string();
    Host {
        server: v(&f.server),
        password: v(&f.password),
        workstation: v(&f.workstation),
        base_path: v(&f.base_path),
        db_user: v(&f.db_user),
        db_password: v(&f.db_password),
        base_url: String::new(),
        dump: String::new(),
        project: v(&f.project),
        tools: Vec::new(),
        mode: String::new(),
    }
}

/// Поля окна отличаются от сохранённых? `dump`, `tools` и `base_url` в окне
/// не редактируются (`host_of` отдаёт их пустыми), поэтому не сравниваются.
pub fn fields_differ(a: &Host, b: &Host) -> bool {
    fn key(h: &Host) -> [&str; 7] {
        [&h.server, &h.workstation, &h.password, &h.base_path, &h.db_user, &h.db_password, &h.project]
    }
    key(a) != key(b)
}

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

impl Panel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Выбор «Системная» подхватывает палитру Windows на каждом открытии
        // окна — смена темы системы видна без перезапуска программы.
        let (kit, host) = {
            let ui = cx.global::<Ui>();
            (ui.kit.clone(), ui.cfg.host.clone())
        };
        if theme::Choice::load(&kit) == theme::Choice::System {
            Theme::sync_system_appearance(None, cx);
        }
        cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() && !cx.global::<Ui>().dialog_open {
                hide_window(window, cx);
            }
        })
        .detach();

        let fields = Fields {
            server: field("192.168.1.10:3393", &host.server, false, window, cx),
            password: field("пароль рабочего места", &host.password, true, window, cx),
            base_path: field("C:\\Bases\\Buh", &host.base_path, false, window, cx),
            db_user: field("Администратор", &host.db_user, false, window, cx),
            db_password: field("пароль", &host.db_password, true, window, cx),
            project: field("C:\\proj\\Buh", &host.project, false, window, cx),
            workstation: field("dev1", &host.workstation, false, window, cx),
        };
        let original = host.clone();
        // Клавиатура сразу у панели: Esc прячет окно и до того, как щёлкнули
        // хоть по одному полю.
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            focus,
            fields,
            log_scroll: ScrollHandle::new(),
            project_scroll: ScrollHandle::new(),
            log_len: 0,
            original,
            viewport_h: None,
            params_h: None,
            last_fit: None,
            fit_queued: false,
        }
    }

    /// Обмер на отрисовке: как только известны и видимая область, и
    /// содержимое «Параметров», окно подбирает высоту на их разницу — не
    /// только в первый раз, а при каждом обмере, заметно отличающемся от
    /// прошлого подбора (`window::needs_fit`). Цикла нет: после
    /// `window.resize` новый обмер даёт пару, для которой `fit_height_for`
    /// возвращает `None` (неподвижная точка или зажатый предел). Сам
    /// подбор — отложенно (`defer_in`): менять размер окна прямо посреди
    /// раскладки нельзя.
    fn measured(&mut self, viewport: Option<Pixels>, params: Option<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.viewport_h = viewport.or(self.viewport_h);
        self.params_h = params.or(self.params_h);
        let (Some(v), Some(c)) = (self.viewport_h, self.params_h) else { return };
        // Два обмера за кадр (видимая область и содержимое) — один подбор:
        // отложенный замыкание читает пару, какой она стала к концу кадра.
        if self.fit_queued || !crate::gui::window::needs_fit(self.last_fit, (v, c)) {
            return;
        }
        self.fit_queued = true;
        cx.defer_in(window, |this, window, cx| {
            this.fit_queued = false;
            let (Some(v), Some(c)) = (this.viewport_h, this.params_h) else { return };
            if !crate::gui::window::needs_fit(this.last_fit, (v, c)) {
                return;
            }
            this.last_fit = Some((v, c));
            crate::gui::window::fit_height(window, cx, c, v);
        });
    }

    /// Область прокрутки с тонкой полосой (Проект, Журнал): корень
    /// `relative`, дорожка-дитя `overflow_y_scroll` + `track_scroll`, полоса
    /// — поверх, абсолютным слоем (как `Scrollable::render` в
    /// `gpui-component`). Дорожка прозрачная, бегунок 3 px (5 px при
    /// наведении), цвета бегунка — из темы (`scrollbar.thumb.background`),
    /// режим показа — из темы. «Параметры» этим не зовёт: вкладка мерная,
    /// окно подстраивается под неё, полосы там нет.
    fn scrolled(id: &'static str, handle: &ScrollHandle, content: impl IntoElement) -> impl IntoElement {
        div()
            .id(id)
            .size_full()
            .relative()
            .child(
                div()
                    .id(format!("{id}-area"))
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(handle)
                    // Полоса лежит поверх области: отступ справа, чтобы
                    // она не ложилась на текст.
                    .pr_2()
                    .child(content),
            )
            .child(div().absolute().inset_0().child(
                Scrollbar::vertical(handle).id(format!("{id}-scrollbar")).viewport_from_layout().styles(|s| {
                    s.track(|t| t.bg(gpui::transparent_black()))
                        .track_hover(|t| t.bg(gpui::transparent_black()))
                        .track_active(|t| t.bg(gpui::transparent_black()))
                        .thumb(|t| t.width(px(3.)).inset(px(2.)).radius(px(2.)))
                        .thumb_hover(|t| t.width(px(5.)).inset(px(1.)).radius(px(3.)))
                        .thumb_active(|t| t.width(px(5.)).inset(px(1.)).radius(px(3.)))
                }),
            ))
    }

    fn is_dirty(&self, cx: &App) -> bool {
        fields_differ(&host_of(&self.fields, cx), &self.original)
    }

    /// Записать параметры в toml и сказать об этом в подвале. Ошибка
    /// проверки — строкой, toml не тронут, набранное остаётся в поле.
    /// Возвращает, удалось ли: по неудаче сеанс не перезапускают.
    fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let host = host_of(&self.fields, cx);
        let r = gui::save_host(cx.global_mut::<Ui>(), host.clone());
        let ok = r.is_ok();
        cx.global_mut::<Ui>().status = Some(match &r {
            Ok(_) => Line { kind: Kind::Ok, text: "Сохранено в 1c-connect.toml".into() },
            Err(e) => Line { kind: Kind::Fail, text: e.clone() },
        });
        if ok {
            self.original = host;
            // Каталог проекта мог смениться — вкладка «Проект» уже про другой.
            gui::refresh_project(cx);
        }
        gui::refresh_ops(cx);
        ok
    }

    /// Выбор папки: на время диалога окно не прячется (`dialog_open`), после
    /// — возвращается на передний план, путь ложится в поле. В toml его
    /// отнесёт «Сохранить»: кнопка загорится сама, поле разошлось с
    /// сохранённым.
    fn pick_dir(&mut self, target: Entity<InputState>, cx: &mut Context<Self>) {
        cx.global_mut::<Ui>().dialog_open = true;
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |_, cx| {
            let picked = rx.await.ok().and_then(|r| r.ok()).flatten().and_then(|v| v.into_iter().next());
            let _ = cx.update(|cx| {
                cx.global_mut::<Ui>().dialog_open = false;
                let ui = cx.global::<Ui>();
                let (Some(w), Some(panel)) = (ui.window, ui.panel.clone()) else { return };
                let _ = w.update(cx, |_, window, cx| {
                    window.activate_window();
                    if let Some(p) = &picked {
                        let text = p.display().to_string();
                        target.update(cx, |s, cx| s.set_value(text, window, cx));
                    }
                    // Клавиатура обратно панели — Esc работает и после диалога.
                    panel.update(cx, |this, cx| this.focus.focus(window, cx));
                });
            });
        })
        .detach();
    }

    /// Поле по Fluent: подпись Caption 12 px приглушённым, 4 px, поле.
    /// Поле по Fluent: без кольца фокуса, у активного поля — акцентная линия
    /// 2 px снизу (как `input:focus` в прежнем `app.css`).
    fn input(state: &Entity<InputState>, accent: Hsla, window: &Window, cx: &App) -> impl IntoElement {
        let focused = state.read(cx).focus_handle(cx).is_focused(window);
        div()
            .relative()
            .child(Input::new(state).focus_bordered(false))
            .when(focused, |d| {
                d.child(div().absolute().bottom_0().left_1().right_1().h(px(2.)).rounded_b_sm().bg(accent))
            })
    }

    fn row(label: &str, caption: Hsla, accent: Hsla, input: &Entity<InputState>, window: &Window, cx: &App) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(div().text_size(px(13.)).text_color(caption).child(label.to_string()))
            .child(Self::input(input, accent, window, cx))
    }

    /// Слой Fluent: заливка слоя, скругление 8, обводка 1 px низкой
    /// непрозрачности, без тени.
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
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let (bg, border, fg, accent) = (t.background, t.border, t.foreground, t.primary);
        let (success, warning, danger, card, window_border) = (t.success, t.warning, t.danger, t.secondary, t.window_border);
        let theme = t.clone();
        // `muted_foreground` отдан подсказкам в полях (его берёт placeholder
        // из темы) и в окне больше нигде не встречается: осветлённый, он не
        // добирал контраста для настоящего текста. Подписи, подробности
        // строк и подстрока карточки — основной текст вполсилы.
        let caption = fg.opacity(0.75);

        let ui = cx.global::<Ui>();
        let (tab, busy, link) = (ui.tab, ui.busy, ui.link.clone());
        let (unread, failed) = (ui.unread, ui.failed_checks);
        let (dot, state, sub) = status_line(&link, &ui.cfg.ports());
        let foot_left = match &ui.status {
            Some(l) => (l.text.clone(), matches!(l.kind, Kind::Fail)),
            None => (ui.kit.root.display().to_string(), false),
        };
        // Полоса хода в разделителе над подвалом: что-то идёт — операция,
        // подключение туннеля или фоновое обновление состояния.
        let working = cx.global::<Ui>().working();

        // Шапка — на поверхности, без отдельной полосы: Body Strong + ✕.
        let header = h_flex()
            .items_center()
            .gap_2()
            .h_9()
            .child(img(icon()).size_6())
            .child(div().font_weight(FontWeight::SEMIBOLD).child("1c-connect"))
            .child(div().flex_1())
            .child(
                Button::new("hide")
                    .ghost()
                    .compact()
                    .label("✕")
                    .tooltip("Скрыть (Esc). Программа остаётся в трее")
                    .on_click(|_, window, cx| hide_window(window, cx)),
            );

        // Карточка состояния — слой. Акцент здесь только у переключателя.
        let status = Self::layer(card, border)
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .p_3()
            .child(div().size_3().flex_shrink_0().rounded_full().bg(Self::dot(dot, &theme)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(state))
                    // Без подстроки («Подключаюсь…») пустая строка всё равно
                    // занимала высоту, и кружок уезжал ниже заголовка.
                    .when(!sub.is_empty(), |v| v.child(div().text_size(px(13.)).text_color(caption).child(sub))),
            )
            .child(
                Switch::new("link").focus_ring(false)
                    .checked(link.is_on())
                    .tooltip("Туннель в среду разработки")
                    .on_change(|_, _, cx| gui::toggle_link(cx)),
            );

        // Бейдж на «Журнале» один: проблемы последней проверки красным, а
        // когда их нет — непрочитанные строки приглушённым слоем (акцент
        // берёт только переключатель и главная кнопка). Ширины сверх самой
        // цифры бейдж не добавляет.
        let log_badge = if failed > 0 {
            Some((failed, danger, bg))
        } else if unread > 0 && tab != gui::Tab::Log {
            Some((unread, border, fg))
        } else {
            None
        };
        let log_tab = match log_badge {
            Some((n, colour, on)) => {
                KitTab::new().label("Журнал").suffix(div().rounded_sm().bg(colour).text_color(on).child(n.to_string()))
            }
            None => KitTab::new().label("Журнал"),
        };
        // Трём подписям хватает обычного размера (`small`, 14 px): на пяти
        // приходилось жать до `xsmall`.
        let tabs = TabBar::new("tabs")
            .underline()
            .small()
            .selected_index(gui::Tab::ALL.iter().position(|t| *t == tab).unwrap_or(0))
            .children([KitTab::new().label("Параметры"), KitTab::new().label("Проект"), log_tab])
            .on_click(cx.listener(|_, i: &usize, _, cx| {
                let Some(t) = gui::Tab::ALL.get(*i).copied() else { return };
                let ui = cx.global_mut::<Ui>();
                ui.tab = t;
                if t == gui::Tab::Log {
                    ui.unread = 0;
                }
                // Состояние git читают при открытии вкладки, как в `app.js`.
                if t == gui::Tab::Project {
                    gui::refresh_project(cx);
                }
                cx.refresh_windows();
            }));

        let pick = |id: &'static str, target: Entity<InputState>, cx: &mut Context<Self>| {
            Button::new(id)
                .outline()
                .label("Выбрать…")
                .on_click(cx.listener(move |this, _, _, cx| this.pick_dir(target.clone(), cx)))
        };
        let f = &self.fields;

        let body: AnyElement = match tab {
            // Обмер до `.id`: `on_children_prepainted` есть у `Div`, а `.id`
            // отдаёт уже `Stateful<Div>`.
            gui::Tab::Params => div()
                // Высота содержимого вкладки: по ней окно подбирает свою.
                .on_children_prepainted({
                    let m = cx.listener(|this: &mut Self, b: &Vec<Bounds<Pixels>>, window, cx| {
                        this.measured(None, b.first().map(|b| b.size.height), window, cx)
                    });
                    move |b, window, cx| m(&b, window, cx)
                })
                .id("params")
                .size_full()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .gap_3()
                        .child(Self::row("Адрес среды и порт SSH", caption, accent, &f.server, window, cx))
                        .child(
                            h_flex()
                                .gap_3()
                                .items_end()
                                .child(div().flex_1().child(Self::row("Рабочее место", caption, accent, &f.workstation, window, cx)))
                                .child(div().flex_1().child(Self::row("Пароль рабочего места", caption, accent, &f.password, window, cx))),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(div().text_size(px(13.)).text_color(caption).child("База"))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().flex_1().child(Self::input(&f.base_path, accent, window, cx)))
                                        .child(pick("pick-base", f.base_path.clone(), cx)),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .child(div().flex_1().child(Self::row("Пользователь базы", caption, accent, &f.db_user, window, cx)))
                                .child(div().flex_1().child(Self::row("Пароль", caption, accent, &f.db_password, window, cx))),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(div().text_size(px(13.)).text_color(caption).child("Каталог исходников проекта"))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().flex_1().child(Self::input(&f.project, accent, window, cx)))
                                        .child(pick("pick-project", f.project.clone(), cx)),
                                ),
                        ),
                )
                .into_any_element(),
            // «Проект» = прежние «Расширение» и «Проект» одной лентой:
            // сверху операции на машине, ниже — каталог и состояние ветки.
            gui::Tab::Project => {
                let ui = cx.global::<Ui>();
                let ops = ui.ops.clone();
                let rows = project_rows(ui.project.as_ref());
                let row = || Self::layer(card, border).flex().flex_row().gap_3().items_center().px_3().py_2();
                let fact = |d: Dot, title: String, detail: String| {
                    row().child(div().size_2().flex_shrink_0().rounded_full().bg(Self::dot(d, &theme))).child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(14.)).child(title))
                            .child(div().text_size(px(13.)).text_color(caption).child(detail)),
                    )
                };
                Self::scrolled(
                    "project",
                    &self.project_scroll,
                    v_flex()
                        // 12 px между операциями и строками проекта: это два
                        // списка, а не один.
                        .gap_3()
                        .child(v_flex().gap_2().children(ops.iter().map(|o| fact(o.dot, o.title.clone(), o.detail.clone()))))
                        .child(v_flex().gap_2().children(rows.into_iter().map(|(d, title, detail)| {
                            // Подсказка — не факт о проекте: мелкий текст без кружка.
                            match d {
                                Some(d) => fact(d, title, detail),
                                None => row().child(div().text_size(px(13.)).text_color(caption).child(title)),
                            }
                        }))),
                )
                .into_any_element()
            }
            gui::Tab::Log => {
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
                            // Заголовок раздела: «{операция} · {HH:MM}» из `head_line`;
                            // на экране операция слева полужирным, время справа
                            // мелким приглушённым (заказчик 2026-10-18).
                            let (title, time) = l.text.rsplit_once(" · ").map(|(t, c)| (t.to_string(), c.to_string())).unwrap_or((l.text.clone(), String::new()));
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
                            // на тёмном сливаются в пёстрый лист (заказчик
                            // 2026-10-18). Служебные строки — приглушённые.
                            // `min_w_0` — иначе flex-ребёнок не уже своего
                            // текста и строка не переносится, а обрезается.
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
        };

        // Подвал: ровно одна акцентная кнопка главного действия, остальное —
        // в меню «Ещё» (как в 1С и в Fluent): значок — три линии (Lucide
        // `menu`, лежит в icons/, ISC). Трём кнопкам в 348 px места не было.
        // Меню закрывается само; пункты зовут те же `gui::*`, что и кнопки.
        // Своё имя на вкладку: меню помнит построенные пункты по имени
        // кнопки, и общее имя подсунуло бы «Журналу» пункты «Проекта».
        let more = |id: &'static str| Button::new(id).ghost().icon(Icon::default().data(MENU_SVG)).tooltip("Ещё");
        let busy_now = busy.is_some();

        let foot_actions: AnyElement = match tab {
            gui::Tab::Params => {
                let dirty = self.is_dirty(cx);
                Button::new("save")
                    .primary()
                    .label("Сохранить")
                    .disabled(!dirty)
                    .on_click(cx.listener(|panel, _, _, cx| {
                        let was_on_cur = cx.global::<Ui>().link.is_on();
                        // Сеанс перезапускаем только по удачной записи: иначе
                        // живой туннель оборвался бы ради непринятых параметров.
                        if panel.save(cx) && was_on_cur {
                            gui::stop_session(cx);
                            if let Err(e) = gui::start_session(cx) {
                                gui::note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                            }
                        }
                        cx.refresh_windows();
                    }))
                    .into_any_element()
            }
            // Главная кнопка ведёт человека по порядку: сначала поставить
            // расширение, потом — выгружать в git. `Busy::Ext` и
            // `Busy::Project` не различают, что именно идёт: на время
            // работы заблокировано всё равно всё.
            gui::Tab::Project => {
                let which = gui::project_primary(&cx.global::<Ui>().ops);
                let (label, hint) = match which {
                    Which::Export => ("Выгрузить", "Выгрузить в git"),
                    _ => ("Установить", "Установить расширение"),
                };
                // В меню — всё, кроме того, что на главной кнопке.
                let rest: Vec<(&'static str, Which)> = [
                    ("Выгрузить в git", Which::Export),
                    ("Загрузить из git", Which::Import),
                    ("Установить расширение", Which::Install),
                    ("Снять расширение", Which::Uninstall),
                ]
                .into_iter()
                .filter(|(_, w)| *w != which)
                .collect();
                h_flex()
                    .gap_2()
                    .child(more("more-project").dropdown_menu(move |mut menu, _, _| {
                        for (label, w) in rest.iter().copied() {
                            menu = menu.item(PopupMenuItem::new(label).disabled(busy_now).on_click(move |_, _, cx| gui::run_op(cx, w)));
                        }
                        menu
                    }))
                    .child(
                        Button::new("primary")
                            .primary()
                            .label(label)
                            .tooltip(hint)
                            .disabled(busy_now)
                            .loading(matches!(busy, Some(Busy::Ext) | Some(Busy::Project)))
                            .on_click(move |_, _, cx| gui::run_op(cx, which)),
                    )
                    .into_any_element()
            }
            gui::Tab::Log => h_flex()
                .gap_2()
                .child(more("more-log").dropdown_menu(move |menu, _, _| {
                    menu.item(PopupMenuItem::new("Открыть журнал").disabled(busy_now).on_click(|_, _, cx| {
                        let path = cx.global::<Ui>().kit.path(gui::LOG_FILE);
                        gui::open_path(cx, &path);
                    }))
                    .item(PopupMenuItem::new("Открыть отчёт last-check.txt").disabled(busy_now).on_click(|_, _, cx| {
                        let path = cx.global::<Ui>().kit.path("last-check.txt");
                        gui::open_path(cx, &path);
                    }))
                    .item(PopupMenuItem::new("Очистить журнал").disabled(busy_now).on_click(|_, _, cx| {
                        let ui = cx.global_mut::<Ui>();
                        ui.log.clear();
                        ui.unread = 0;
                        cx.refresh_windows();
                    }))
                }))
                .child(
                    Button::new("check")
                        .primary()
                        .label("Проверить")
                        .disabled(busy_now)
                        .loading(busy == Some(Busy::Check))
                        .on_click(|_, _, cx| gui::run_check(cx)),
                )
                .into_any_element(),
        };

        v_flex()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    hide_window(window, cx);
                }
            }))
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_size(px(15.))
            .border_1()
            .border_color(window_border)
            .rounded_lg()
            .overflow_hidden()
            .p_4()
            .gap_3()
            .child(header)
            .child(status)
            .child(div().border_b_1().border_color(border.opacity(0.6)).child(tabs))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // Видимая область тела вкладки — вторая половина обмера.
                    .on_children_prepainted({
                        let m = cx.listener(|this: &mut Self, b: &Vec<Bounds<Pixels>>, window, cx| {
                            this.measured(b.first().map(|b| b.size.height), None, window, cx)
                        });
                        move |b, window, cx| m(&b, window, cx)
                    })
                    .child(body),
            )
            .child(if working {
                // Работа: дорожка акцентом темы, бегунок въезжает слева и
                // уезжает справа по кругу. Возврат в линию — мгновенный,
                // затухания нет (решение контролёра 2026-10-18).
                div().w_full().h(px(2.)).relative().bg(border.opacity(0.6)).child(
                    div()
                        .absolute()
                        .top_0()
                        .h(px(2.))
                        .w(relative(0.25))
                        .rounded_full()
                        .bg(accent)
                        .with_animation(
                            "progress-thumb",
                            Animation::new(Duration::from_millis(1200)).repeat(),
                            |d, t| d.left(relative(-0.25 + 1.25 * t)),
                        ),
                )
            } else {
                // Покой: та же линия 1 px, прижата к низу 2 px полосы —
                // высота разделителя не меняется между состояниями.
                div().w_full().h(px(2.)).child(div().w_full().h(px(1.)).mt(px(1.)).bg(border.opacity(0.6)))
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .pt_3()
                    // Итог последнего действия переносится (до двух строк),
                    // а не обрезается многоточием: плашка растёт по тексту.
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .line_height(relative(1.2))
                            .text_color(if foot_left.1 { danger } else { caption })
                            .whitespace_normal()
                            .line_clamp(2)
                            .child(foot_left.0),
                    )
                    .child(foot_actions),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{fields_differ, project_rows, status_line, HINT_PROJECT};
    use crate::cfg::{self, Host};
    use crate::gui::{Dot, ProjectView};
    use crate::session::State;
    use std::time::Instant;

    #[test]
    fn status_line_reads_like_the_old_window() {
        let p = cfg::Ports { mcp: 7771, vrunner: 7781, web: 7791 };
        assert_eq!(status_line(&State::Off, &p), (Dot::Off, "Не подключён", "туннель в среду разработки выключен".into()));
        assert_eq!(status_line(&State::Failed("нет сети".into()), &p), (Dot::Down, "Не подключён", "нет сети".into()));
        let (d, t, s) = status_line(&State::Up { since: Instant::now() }, &p);
        assert_eq!((d, t), (Dot::Up, "Подключён"));
        assert_eq!(s, "0 мин · mcp-1c 7771 · web 7791");
    }

    #[test]
    fn dirty_ignores_dump_tools_and_base_url_but_sees_a_changed_field() {
        let saved = Host { dump: "C:\\proj\\src".into(), tools: vec!["x".into()], ..Host::default() };
        // Поля «Адрес публикации» в окне нет — `host_of` отдаёт его пустым.
        let shown = Host { dump: String::new(), tools: Vec::new(), base_url: String::new(), ..saved.clone() };
        assert!(!fields_differ(&shown, &saved));
        let edited = Host { server: "h:1".into(), ..shown.clone() };
        assert!(fields_differ(&edited, &saved));
        // Каталог проекта теперь в форме — его правка видна.
        let moved = Host { project: "D:\\other\\Buh".into(), ..shown };
        assert!(fields_differ(&moved, &saved));
    }

    #[test]
    fn project_rows_explain_a_missing_layout_and_list_status_lines() {
        let none = project_rows(None);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0], (None, "Каталог проекта задаётся в «Параметрах»; install раскладывает git.".into(), String::new()));

        let bad = ProjectView {
            dir: "C:\\proj\\Buh".into(),
            branch: "dev1".into(),
            lines: Vec::new(),
            ok: false,
            why: "каталог проекта ещё не разложен".into(),
        };
        let rows = project_rows(Some(&bad));
        assert_eq!(rows[0], (Some(Dot::Busy), "C:\\proj\\Buh".into(), "каталог проекта ещё не разложен".into()));
        // Подсказка — без кружка: вид рисует её мелким текстом.
        assert_eq!(rows[1], (None, HINT_PROJECT.into(), String::new()));

        let good = ProjectView {
            lines: vec!["ветка: dev1".into(), "изменено: 2 файла".into()],
            ok: true,
            why: String::new(),
            ..bad
        };
        let rows = project_rows(Some(&good));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], (Some(Dot::Up), "C:\\proj\\Buh".into(), "ветка dev1".into()));
        assert_eq!(rows[1], (Some(Dot::Off), "ветка".into(), "dev1".into()));
        assert_eq!(rows[2], (Some(Dot::Off), "изменено".into(), "2 файла".into()));
    }

    #[test]
    fn the_hint_no_longer_points_at_a_tab_that_is_gone() {
        assert!(!HINT_PROJECT.contains("Расширение"), "{HINT_PROJECT}");
    }
}
