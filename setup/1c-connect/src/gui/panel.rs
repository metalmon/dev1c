//! Содержимое флайаута: шапка, карточка состояния, вкладки-пивот,
//! параметры с кнопкой «Сохранить», список операций, проверка, журнал и подвал.
//!
//! Окно пересобирается на каждый показ, поэтому `Panel` держит только
//! сущности полей, фокус и прокрутку журнала; всё остальное читается из
//! глобала `Ui` в `render`. Изменили `Ui` — `cx.refresh_windows()`.

use crate::cfg::{self, Host};
use crate::gui::theme;
use crate::gui::window::hide_window;
use crate::gui::{self, Busy, Dot, Kind, Line, Ui};
use crate::session::State;
use common::check::Check;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Disableable, FocusableExt, IndexPath, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::Duration;

/// Значок программы в шапке — тот же PNG, что у окна и у трея.
const ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");

/// Рабочие места среды: ровно три логина, больше `cfg::validate` не пустит.
const WORKSTATIONS: [&str; 3] = ["dev1", "dev2", "dev3"];

/// Как часто перерисовывать окно, чтобы «N мин» в карточке не отставало.
const TICK: Duration = Duration::from_secs(30);

type WsState = Entity<SelectState<Vec<&'static str>>>;

fn icon() -> Arc<Image> {
    static I: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    I.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, ICON_PNG.to_vec()))).clone()
}

/// Поля вкладки «Параметры». Рабочее место — `Select` из `gpui-component`:
/// три значения обходятся одной сущностью состояния, сегментные вкладки не
/// понадобились.
pub struct Fields {
    pub server: Entity<InputState>,
    pub password: Entity<InputState>,
    pub base_path: Entity<InputState>,
    pub db_user: Entity<InputState>,
    pub db_password: Entity<InputState>,
    pub base_url: Entity<InputState>,
    pub dump: Entity<InputState>,
    pub workstation: WsState,
}

pub struct Panel {
    pub focus: FocusHandle,
    pub fields: Fields,
    log_scroll: ScrollHandle,
    /// Сколько строк журнала было на прошлой отрисовке: чтобы
    /// толкать полосу только на новые, иначе целиком вниз.
    log_len: usize,
    original: Host,
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

/// Информационный факт — не приговор: серый кружок, а не красный.
pub fn check_dot(c: &Check) -> Dot {
    match (c.ok, c.info) {
        (true, false) => Dot::Up,
        (true, true) => Dot::Off,
        _ => Dot::Down,
    }
}

/// Параметры из полей. `project` и `tools` пустые — их подставит `save_host`
/// из текущих, в окне этих полей нет.
pub fn host_of(f: &Fields, cx: &App) -> Host {
    let v = |e: &Entity<InputState>| e.read(cx).value().to_string();
    Host {
        server: v(&f.server),
        password: v(&f.password),
        workstation: f.workstation.read(cx).selected_value().copied().unwrap_or(WORKSTATIONS[0]).to_string(),
        base_path: v(&f.base_path),
        db_user: v(&f.db_user),
        db_password: v(&f.db_password),
        base_url: v(&f.base_url),
        dump: v(&f.dump),
        project: String::new(),
        tools: Vec::new(),
    }
}

/// Поля окна отличаются от сохранённых? `project` и `tools` в окне не
/// редактируются (`host_of` отдаёт их пустыми), поэтому не сравниваются.
pub fn fields_differ(a: &Host, b: &Host) -> bool {
    fn key(h: &Host) -> [&str; 8] {
        [&h.server, &h.workstation, &h.password, &h.base_path, &h.db_user, &h.db_password, &h.base_url, &h.dump]
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

        let ws_ix = WORKSTATIONS.iter().position(|w| *w == host.workstation).unwrap_or(0);
        let fields = Fields {
            server: field("192.168.1.10:3393", &host.server, false, window, cx),
            password: field("пароль рабочего места", &host.password, true, window, cx),
            base_path: field("C:\\Bases\\Buh", &host.base_path, false, window, cx),
            db_user: field("Администратор", &host.db_user, false, window, cx),
            db_password: field("пароль", &host.db_password, true, window, cx),
            base_url: field("http://localhost/Buh/hs/mcp-1c", &host.base_url, false, window, cx),
            dump: field("C:\\proj\\Buh\\src", &host.dump, false, window, cx),
            workstation: cx.new(|cx| SelectState::new(WORKSTATIONS.to_vec(), Some(IndexPath::new(ws_ix)), window, cx)),
        };
        // Минуты в карточке: пока окно живо, раз в полминуты перерисовка.
        // Окно закрыли — сущность не поднимается, задача уходит.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        let original = host.clone();
        Self { focus: cx.focus_handle(), fields, log_scroll: ScrollHandle::new(), log_len: 0, original }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        fields_differ(&host_of(&self.fields, cx), &self.original)
    }

    /// Записать параметры в toml и сказать об этом в подвале. Ошибка
    /// проверки — строкой, toml не тронут, набранное остаётся в поле.
    fn save(&mut self, cx: &mut Context<Self>) {
        let host = host_of(&self.fields, cx);
        let r = gui::save_host(cx.global_mut::<Ui>(), host.clone());
        let ok = r.is_ok();
        cx.global_mut::<Ui>().status = Some(match &r {
            Ok(_) => Line { kind: Kind::Ok, text: "Сохранено в 1c-connect.toml".into() },
            Err(e) => Line { kind: Kind::Fail, text: e.clone() },
        });
        if ok {
            self.original = host;
        }
        gui::refresh_ops(cx);
    }

    /// Выбор папки: на время диалога окно не прячется (`dialog_open`), после
    /// — возвращается на передний план, путь ложится в поле и в toml.
    fn pick_dir(&mut self, target: Entity<InputState>, cx: &mut Context<Self>) {
        cx.global_mut::<Ui>().dialog_open = true;
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |_, cx| {
            let picked = rx.await.ok().and_then(|r| r.ok()).flatten().and_then(|v| v.into_iter().next());
            let _ = cx.update(|cx| {
                cx.global_mut::<Ui>().dialog_open = false;
                let Some(w) = cx.global::<Ui>().window else { return };
                let _ = w.update(cx, |panel, window, cx| {
                    window.activate_window();
                    if let Some(p) = &picked {
                        let text = p.display().to_string();
                        target.update(cx, |s, cx| s.set_value(text, window, cx));
                        panel.save(cx);
                    }
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

    fn row(label: &str, muted: Hsla, accent: Hsla, input: &Entity<InputState>, window: &Window, cx: &App) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(div().text_size(px(13.)).text_color(muted).child(label.to_string()))
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
        let (bg, border, fg, muted, accent) = (t.background, t.border, t.foreground, t.muted_foreground, t.primary);
        let (success, warning, danger, card, window_border) = (t.success, t.warning, t.danger, t.secondary, t.window_border);
        let theme = t.clone();

        let ui = cx.global::<Ui>();
        let (tab, busy, link) = (ui.tab, ui.busy, ui.link.clone());
        let (unread, failed) = (ui.unread, ui.failed_checks);
        let (dot, state, sub) = status_line(&link, &ui.cfg.ports());
        let foot_left = match &ui.status {
            Some(l) => (l.text.clone(), matches!(l.kind, Kind::Fail)),
            None => (ui.kit.root.display().to_string(), false),
        };

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
                    .child(div().text_size(px(13.)).text_color(muted).child(sub)),
            )
            .child(
                Switch::new("link")
                    .checked(link.is_on())
                    .tooltip("Туннель в среду разработки")
                    .on_change(|_, _, cx| gui::toggle_link(cx)),
            );

        // Бейдж — не акцент: проблемы красным, непрочитанное приглушённым
        // слоем (акцент берёт только переключатель и главная кнопка).
        let badge = |n: usize, colour: Hsla, on: Hsla| {
            div().px_1().rounded_sm().bg(colour).text_size(px(13.)).text_color(on).child(n.to_string())
        };
        let with_badge = |label: &'static str, n: usize, colour: Hsla, on: Hsla| {
            let t = KitTab::new().label(label);
            if n > 0 {
                t.suffix(badge(n, colour, on))
            } else {
                t
            }
        };
        let tabs = TabBar::new("tabs")
            .underline()
            .selected_index(match tab {
                gui::Tab::Params => 0,
                gui::Tab::Ext => 1,
                gui::Tab::Check => 2,
                gui::Tab::Log => 3,
            })
            .children([
                KitTab::new().label("Параметры"),
                KitTab::new().label("Расширение"),
                with_badge("Проверка", failed, danger, bg),
                with_badge("Журнал", if tab == gui::Tab::Log { 0 } else { unread }, border, fg),
            ])
            .on_click(cx.listener(|_, i: &usize, _, cx| {
                let t = [gui::Tab::Params, gui::Tab::Ext, gui::Tab::Check, gui::Tab::Log][*i];
                let ui = cx.global_mut::<Ui>();
                ui.tab = t;
                if t == gui::Tab::Log {
                    ui.unread = 0;
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
            gui::Tab::Params => div()
                .id("params")
                .size_full()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .gap_3()
                        .child(Self::row("Адрес среды и порт SSH", muted, accent, &f.server, window, cx))
                        .child(
                            h_flex()
                                .gap_3()
                                .items_end()
                                .child(
                                    div().flex_1().child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_size(px(13.)).text_color(muted).child("Рабочее место"))
                                            .child(Select::new(&f.workstation).focus_ring(false)),
                                    ),
                                )
                                .child(div().flex_1().child(Self::row("Пароль рабочего места", muted, accent, &f.password, window, cx))),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(div().text_size(px(13.)).text_color(muted).child("База"))
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
                                .child(div().flex_1().child(Self::row("Пользователь базы", muted, accent, &f.db_user, window, cx)))
                                .child(div().flex_1().child(Self::row("Пароль", muted, accent, &f.db_password, window, cx))),
                        )
                        .child(Self::row("Адрес публикации", muted, accent, &f.base_url, window, cx))
                        .child(
                            v_flex()
                                .gap_1()
                                .child(div().text_size(px(13.)).text_color(muted).child("Каталог выгрузки конфигурации"))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().flex_1().child(Self::input(&f.dump, accent, window, cx)))
                                        .child(pick("pick-dump", f.dump.clone(), cx)),
                                ),
                        ),
                )
                .into_any_element(),
            gui::Tab::Ext => div()
                .id("ext")
                .size_full()
                .overflow_y_scroll()
                .child(v_flex().gap_2().children(cx.global::<Ui>().ops.iter().map(|o| {
                    Self::layer(card, border)
                        .flex()
                        .flex_row()
                        .gap_3()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(div().size_2().flex_shrink_0().rounded_full().bg(Self::dot(o.dot, &theme)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(o.title.clone())
                                .child(div().text_size(px(13.)).text_color(muted).child(o.detail.clone())),
                        )
                })))
                .into_any_element(),
            gui::Tab::Check => {
                let checks = cx.global::<Ui>().checks.clone();
                div()
                    .id("check")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap_2()
                            .when(checks.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_color(muted)
                                        .child("Семь фактов снизу вверх; отчёт — last-check.txt в комплекте."),
                                )
                            })
                            .children(checks.iter().map(|c| {
                                let d = check_dot(c);
                                Self::layer(card, border)
                                    .flex()
                                    .flex_row()
                                    .gap_3()
                                    .px_3()
                                    .py_2()
                                    .child(div().size_2().flex_shrink_0().rounded_full().mt_1p5().bg(Self::dot(d, &theme)))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(c.what.clone())
                                            .child(div().text_size(px(13.)).text_color(muted).child(c.detail.clone()))
                                            .when(d == Dot::Down && !c.hint.is_empty(), |v| {
                                                v.child(div().text_size(px(13.)).text_color(warning).child(c.hint.clone()))
                                            }),
                                    )
                            })),
                    )
                    .into_any_element()
            }
            gui::Tab::Log => {
                let lines = cx.global::<Ui>().log.clone();
                if lines.len() != self.log_len {
                    self.log_len = lines.len();
                    self.log_scroll.scroll_to_bottom();
                }
                div()
                    .id("log")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.log_scroll)
                    .font_family("Lilex")
                    .child(v_flex().children(lines.into_iter().map(|l| {
                        div()
                            .text_color(match l.kind {
                                Kind::Ok => success,
                                Kind::Warn => warning,
                                Kind::Fail => danger,
                                Kind::Say => fg,
                            })
                            .child(l.text)
                    })))
                    .into_any_element()
            }
        };

        // Одна акцентная кнопка на вкладку; остальные — «standard».
        let foot_actions: AnyElement = match tab {
            gui::Tab::Params => {
                let dirty = self.is_dirty(cx);
                Button::new("save")
                    .primary()
                    .label("Сохранить")
                    .disabled(!dirty)
                    .on_click(cx.listener(|panel, _, _, cx| {
                        let was_on_cur = cx.global::<Ui>().link.is_on();
                        panel.save(cx);
                        if was_on_cur {
                            gui::stop_session(cx);
                            let _ = gui::start_session(cx);
                        }
                        cx.refresh_windows();
                    }))
                    .into_any_element()
            }
            // `Busy::Ext` не различает установку и снятие, поэтому спиннер
            // на обеих — обе всё равно заблокированы на время работы.
            gui::Tab::Ext => h_flex()
                .gap_2()
                .child(
                    Button::new("uninstall")
                        .outline()
                        .label("Снять")
                        .disabled(busy.is_some())
                        .loading(busy == Some(Busy::Ext))
                        .on_click(|_, _, cx| gui::run_op(cx, true)),
                )
                .child(
                    Button::new("install")
                        .primary()
                        .label("Установить")
                        .disabled(busy.is_some())
                        .loading(busy == Some(Busy::Ext))
                        .on_click(|_, _, cx| gui::run_op(cx, false)),
                )
                .into_any_element(),
            gui::Tab::Check => Button::new("check")
                .primary()
                .label("Проверить")
                .disabled(busy.is_some())
                .loading(busy == Some(Busy::Check))
                .on_click(|_, _, cx| gui::run_check(cx))
                .into_any_element(),
            gui::Tab::Log => Button::new("clear")
                .outline()
                .label("Очистить")
                .on_click(|_, _, cx| {
                    let ui = cx.global_mut::<Ui>();
                    ui.log.clear();
                    ui.unread = 0;
                    cx.refresh_windows();
                })
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
            .child(div().flex_1().min_h_0().child(body))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(border.opacity(0.6))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(if foot_left.1 { danger } else { muted })
                            .truncate()
                            .child(foot_left.0),
                    )
                    .child(foot_actions),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{check_dot, fields_differ, status_line};
    use crate::cfg::{self, Host};
    use crate::gui::Dot;
    use crate::session::State;
    use common::check::Check;
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
    fn dirty_ignores_project_and_tools_but_sees_a_changed_field() {
        let saved = Host { project: "C:\\proj".into(), tools: vec!["x".into()], ..Host::default() };
        let shown = Host { project: String::new(), tools: Vec::new(), ..saved.clone() };
        assert!(!fields_differ(&shown, &saved));
        let edited = Host { server: "h:1".into(), ..shown };
        assert!(fields_differ(&edited, &saved));
    }

    #[test]
    fn check_dots_tell_fail_from_note() {
        assert_eq!(check_dot(&Check::pass("a", "")), Dot::Up);
        assert_eq!(check_dot(&Check::note("a", "")), Dot::Off);
        assert_eq!(check_dot(&Check::fail("a", "", "")), Dot::Down);
    }
}
