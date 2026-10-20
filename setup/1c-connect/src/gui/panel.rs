//! Содержимое флайаута: шапка, карточка состояния, три вкладки-пивота
//! (Параметры, Проект, Журнал), тело вкладки и подвал — слева итог
//! последнего действия, справа одна акцентная кнопка главного действия и
//! кнопка «…» с меню остального.
//!
//! Окно пересобирается на каждый показ, поэтому `Panel` держит только
//! сущности полей, фокус и прокрутку журнала; всё остальное читается из
//! глобала `Ui` в `render`. Изменили `Ui` — `cx.refresh_windows()`.

use crate::cfg::Host;
use crate::gui::theme;
use crate::gui::window::hide_window;
use crate::gui::{self, Busy, Dot, Kind, Line, ProjectView, Readiness, Running, Ui, Which};
use crate::session::State;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Disableable, FocusableExt, Icon, Sizable, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// Карточка состояния: кружок, заголовок, можно ли «Прервать» — всегда одна
/// строка высотой. Подстрок, заметок и застоя в карточке нет (заказчик
/// 2026-10-19: прыгающая высота окна) — застой уходит отдельной строкой в
/// журнал (`gui::stale_line`), остальное и так дублируется там же.
pub struct Card {
    pub dot: Dot,
    pub title: String,
    pub cancel: bool,
}

/// Заголовок — по `cfg.host.mode`: в локальном режиме речь про MCP (слушатели
/// держат порты сами, см. `session::start`), а не про связь со средой; отказ
/// там же показывает саму причину (порт занят и т.п.), а не общее «не
/// подключён». В туннельном режиме — как раньше.
///
/// `ready` — вердикт `verify::readiness`, посчитанный отдельно и не на
/// главном потоке (`gui::run_readiness`): пока связь/MCP не «включены»,
/// карточка его не смотрит вовсе — выключенный или подключающийся переключатель
/// ничего не обещает агенту, смотреть там нечего. Зелёная «MCP включены»/
/// «Подключён» становится жёлтой, только когда готовность реально
/// провалилась (`Readiness::Warn`) — `Unknown` (поток ещё считает) и `Ok`
/// показывают прежний зелёный текст, значение переключателя при этом не
/// меняется: это по-прежнему «включить/выключить обслуживание вызовов».
pub fn card(progress: Option<&Running>, link: &State, mode: &str, ready: &Readiness, now: Instant) -> Card {
    if let Some(r) = progress {
        let (_, clock) = gui::progress_text(r, now);
        let title = format!("{} · {}", r.phase.title(), clock);
        return Card { dot: Dot::Busy, title, cancel: gui::can_cancel(r.phase) };
    }
    let local = mode == "local";
    // Текст «MCP включены, но …» — местная формулировка (заказчик и дефект
    // целиком про локальный режим, см. журнал модуля); туннельный режим
    // показывает прежнее «Подключён» — его собственный провал публикации уже
    // становится `State::Failed` (красным) в `Ev::Publish`, второй жёлтый
    // смысл ему не нужен.
    if local {
        if let (State::Up { .. }, Readiness::Warn(check)) = (link, ready) {
            // Коротко: в карточке одна строка на 230 точек, и длинный заголовок
            // обрезался на «но агент б…». Положение переключателя и так говорит,
            // что MCP включены, поэтому заголовок отвечает на главный вопрос.
            let title = format!("Агент базу не увидит: {}", gui::readiness_reason(check));
            return Card { dot: Dot::Warn, title, cancel: false };
        }
    }
    let (dot, title): (Dot, String) = match link {
        State::Off => (Dot::Off, if local { "MCP выключены".into() } else { "Не подключён".into() }),
        State::Connecting => (Dot::Busy, if local { "Включаю…".into() } else { "Подключаюсь…".into() }),
        State::Up { .. } => (Dot::Up, if local { "MCP включены".into() } else { "Подключён".into() }),
        State::Retrying(_) => (Dot::Busy, "Переподключаюсь".into()),
        State::Failed(why) => (Dot::Down, if local { why.clone() } else { "Не подключён".into() }),
    };
    Card { dot, title, cancel: false }
}

/// Можно ли нажать «Сохранить»: поля отличаются от сохранённых, и прямо
/// сейчас не идёт операция/проверка и не подключается/перезапускается сеанс —
/// правка не должна тронуть toml и дёрнуть сеанс посреди них.
pub fn can_save(dirty: bool, busy: bool, progress: bool) -> bool {
    dirty && !busy && !progress
}

/// Почему «Сохранить» недоступна — словами разработчика 1С, а не молчанием
/// (заказчик 2026-10-20: кнопка просто гаснет, непонятно почему). Ровно те
/// же условия, что держит `can_save` — `busy` тут и есть «операция или
/// проверка идёт» (`can_save`'s `busy || progress`; идущая операция всегда
/// держит и то, и другое, см. `run_op`). `op` называет операцию, когда она
/// идёт — экспорт и импорт говорят разное, расширение (установка/снятие)
/// общо; `None` при занятости значит идёт проверка, а не операция.
/// `None` результата — кнопку можно нажать, объяснять нечего.
pub fn save_hint(dirty: bool, busy: bool, op: Option<Which>) -> Option<String> {
    if !dirty {
        return Some("Нечего сохранять — поля ещё не менялись".into());
    }
    if !busy {
        return None;
    }
    let what = match op {
        Some(Which::Export) => "выгрузка из 1С",
        Some(Which::Import) => "загрузка в 1С",
        Some(Which::Install) | Some(Which::Uninstall) => "работа с расширением",
        None => "проверка",
    };
    Some(format!("Подождите: идёт {what}"))
}

/// Левый слот подвала: ровно один текст, не два (заказчик 2026-10-20: «возле
/// нижней кнопки почему-то две надписи стало»). Порядок, кто в нём говорит:
/// настоящее сообщение (итог сохранения, ошибка) важнее подсказки, почему
/// «Сохранить» недоступна, а та — важнее пути комплекта, который слот
/// показывает, когда сказать больше нечего. Обрезка в одну строку и
/// подсказка по наведению — дело `render`, здесь только выбор источника.
pub fn foot_left_text(status: Option<&Line>, hint: Option<&str>, kit_path: &str) -> (String, bool) {
    if let Some(l) = status {
        return (l.text.clone(), matches!(l.kind, Kind::Fail));
    }
    if let Some(h) = hint {
        return (h.to_string(), false);
    }
    (kit_path.to_string(), false)
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

/// Поля вкладки «Параметры», которые режим реально показывает. В локальном
/// режиме коннектор слушает loopback, а не поднимает туннель в среду —
/// адрес среды, пароль рабочего места и имя рабочего места ничего не меняют,
/// и показывать их незачем. Всё, что не `"local"`, — режим `tunnel`.
/// Отрисовка рядов и сравнение «изменилось ли» ходят только через этот список.
pub fn shown_fields(mode: &str) -> &'static [&'static str] {
    if mode == "local" {
        &["base_path", "db_user", "db_password", "project"]
    } else {
        &["server", "workstation", "password", "base_path", "db_user", "db_password", "project"]
    }
}

fn field_value<'a>(h: &'a Host, name: &str) -> &'a str {
    match name {
        "server" => &h.server,
        "password" => &h.password,
        "workstation" => &h.workstation,
        "base_path" => &h.base_path,
        "db_user" => &h.db_user,
        "db_password" => &h.db_password,
        "project" => &h.project,
        _ => "",
    }
}

/// Параметры из полей. `dump`, `tools`, `mode`, `publication`, `http_port` и
/// `base_url` пустые — их подставит `save_host` из текущих, в окне этих полей
/// нет. Поля, которых режим не показывает, тоже пустые — набранное в них
/// (если там что-то осталось от прежнего режима) в toml не попадёт;
/// `Panel::save` сам переносит рабочее место, адрес среды и пароль рабочего
/// места из текущих параметров, как `save_host` уже делает для
/// `dump`/`tools`/`mode`/`publication`/`http_port`.
pub fn host_of(f: &Fields, mode: &str, cx: &App) -> Host {
    let shown = shown_fields(mode);
    let v = |name: &str, e: &Entity<InputState>| {
        if shown.contains(&name) { e.read(cx).value().to_string() } else { String::new() }
    };
    Host {
        server: v("server", &f.server),
        password: v("password", &f.password),
        workstation: v("workstation", &f.workstation),
        base_path: v("base_path", &f.base_path),
        db_user: v("db_user", &f.db_user),
        db_password: v("db_password", &f.db_password),
        base_url: String::new(),
        dump: String::new(),
        project: v("project", &f.project),
        tools: Vec::new(),
        mode: String::new(),
        publication: String::new(),
        http_port: None,
    }
}

/// Поля окна отличаются от сохранённых? Сравниваются только те поля, что
/// режим показывает: правка скрытого поля (адрес среды, пароль рабочего
/// места, рабочее место в локальном режиме) не должна зажигать «Сохранить» —
/// оно ни на что не влияет, пока режим не сменился. `dump`, `tools`,
/// `base_url` и `mode` в окне не редактируются (`host_of` отдаёт их
/// пустыми/нулями), поэтому и так не входят в список.
pub fn fields_differ(a: &Host, b: &Host, mode: &str) -> bool {
    shown_fields(mode).iter().any(|f| field_value(a, f) != field_value(b, f))
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
        let mode = cx.global::<Ui>().cfg.host.mode.clone();
        fields_differ(&host_of(&self.fields, &mode, cx), &self.original, &mode)
    }

    /// Записать параметры в toml и сказать об этом в подвале. Ошибка
    /// Поля формы как `Host`: в локальном режиме скрытые поля переносятся из
    /// текущих параметров. Отделено от записи, чтобы кнопка могла сперва
    /// спросить подтверждение смены базы и только потом писать.
    fn host_from_fields(&self, cx: &mut Context<Self>) -> Host {
        let mode = cx.global::<Ui>().cfg.host.mode.clone();
        let mut host = host_of(&self.fields, &mode, cx);
        if mode == "local" {
            // Адрес среды, пароль рабочего места и имя рабочего места скрыты
            // в этом режиме — переносим их из текущих параметров, как
            // `save_host` уже делает для `dump`/`tools`/`mode`/`base_url`.
            // Рабочее место обязано остаться в toml: от него `cfg::Cfg::ports`
            // считает порты 7770+N и 7780+N.
            let current = cx.global::<Ui>().cfg.host.clone();
            host.server = current.server;
            host.password = current.password;
            host.workstation = current.workstation;
        }
        host
    }

    /// Запись в toml и всё, что за ней следует: итог в подвал, обновление
    /// вкладок, перезапуск сеанса внутри `gui::save_host`.
    fn write(&mut self, host: Host, cx: &mut Context<Self>) -> bool {
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
            Dot::Warn => t.warning,
        }
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let (bg, border, fg, accent) = (t.background, t.border, t.foreground, t.primary);
        let (success, warning, danger, card_bg, window_border) = (t.success, t.warning, t.danger, t.secondary, t.window_border);
        let theme = t.clone();
        // `muted_foreground` отдан подсказкам в полях (его берёт placeholder
        // из темы) и в окне больше нигде не встречается: осветлённый, он не
        // добирал контраста для настоящего текста. Подписи, подробности
        // строк и подстрока карточки — основной текст вполсилы.
        let caption = fg.opacity(0.75);

        let ui = cx.global::<Ui>();
        let (tab, busy, link) = (ui.tab, ui.busy, ui.link.clone());
        let ready = ui.ready.clone();
        let (unread, failed) = (ui.unread, ui.failed_checks);
        let has_progress = ui.progress.is_some();
        // Какая операция идёт, если идёт — для текста под «Сохранить»
        // (`save_hint`): экспорт, импорт и расширение называются разным
        // словом, идущая проверка им не является (`run_op`/`run_check`
        // выставляют разное).
        let running_op = ui.progress.as_ref().map(|r| r.which);
        let mode = ui.cfg.host.mode.clone();
        let c = card(ui.progress.as_ref(), &link, &mode, &ready, Instant::now());
        // Причина недоступности «Сохранить» — только пока вкладка «Параметры»
        // её вообще показывает (на других вкладках кнопки «Сохранить» нет,
        // подсказке говорить не о чем). Один расчёт на весь `render`: тот же
        // `hint` уходит и в левый слот подвала, и в тултип кнопки ниже.
        let busy_now = busy.is_some();
        let dirty = self.is_dirty(cx);
        let hint = (tab == gui::Tab::Params).then(|| save_hint(dirty, busy_now, running_op)).flatten();
        let foot_left = foot_left_text(ui.status.as_ref(), hint.as_deref(), &ui.kit.root.display().to_string());
        let shown = shown_fields(&mode);
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

        // Карточка состояния — слой, всегда одна строка (заказчик 2026-10-19:
        // прыгающая высота окна из-за переменного числа строк). Акцент здесь
        // только у переключателя. На ходу операции переключатель заблокирован
        // (`Switch::disabled`): дёргать туннель посреди git-обмена незачем.
        let status = Self::layer(card_bg, border)
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .p_3()
            .child(div().size_3().flex_shrink_0().rounded_full().bg(Self::dot(c.dot, &theme)))
            .child({
                // Имя базы дописывается в тот же заголовок через « · », как
                // уже фаза с часами выше: у карточки один слот под текст,
                // второй строки это не прибавляет (заказчик 2026-10-19).
                // Базу здесь не пишем: длинное имя сюда не влезет, а человек
                // видит её в поле «База» на «Параметрах» (заказчик 2026-10-20).
                div()
                    .id("status-title")
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(c.title.clone())
            })
            // «Прервать» — перед переключателем, только пока можно отменить
            // (git-обмен; cf_decompile/cf_load не прерываются).
            .when(c.cancel, |d| {
                d.child(
                    Button::new("cancel")
                        .ghost()
                        .small()
                        .label("Прервать")
                        .on_click(|_, _, cx| gui::cancel_op(cx)),
                )
            })
            .child(
                Switch::new("link").focus_ring(false)
                    .checked(link.is_on())
                    .disabled(has_progress)
                    .tooltip(if mode == "local" { "Обслуживание вызовов агента (MCP)" } else { "Туннель в среду разработки" })
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
                .child({
                    // В локальном режиме коннектор слушает loopback, а не
                    // поднимает туннель в среду: адрес среды, рабочее место и
                    // его пароль ничего не меняют — показывать их незачем
                    // (поле без действия роняет доверие к экрану параметров).
                    // Без них ряд не рисуется вовсе — ни дыры, ни подписи
                    // без поля: высота окна не прыгает между режимами, она
                    // просто меньше.
                    let mut col = v_flex().gap_3();
                    if shown.contains(&"server") {
                        col = col.child(Self::row("Адрес среды и порт SSH", caption, accent, &f.server, window, cx));
                    }
                    if shown.contains(&"workstation") {
                        col = col.child(
                            h_flex()
                                .gap_3()
                                .items_end()
                                .child(div().flex_1().child(Self::row("Рабочее место", caption, accent, &f.workstation, window, cx)))
                                .child(div().flex_1().child(Self::row("Пароль рабочего места", caption, accent, &f.password, window, cx))),
                        );
                    }
                    col.child(
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
                    )
                })
                .into_any_element(),
            // «Проект» = прежние «Расширение» и «Проект» одной лентой:
            // сверху операции на машине, ниже — каталог и состояние ветки.
            gui::Tab::Project => {
                let ui = cx.global::<Ui>();
                let ops = ui.ops.clone();
                let cfg = ui.cfg.clone();
                let rows = project_rows(ui.project.as_ref());
                let row = || Self::layer(card_bg, border).flex().flex_row().gap_3().items_center().px_3().py_2();
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
                        .child(v_flex().gap_2().children(ops.iter().map(|o| {
                            // Строка публикации — словами разработчика 1С
                            // (адрес, не дескрипторы/httpd.conf): та же
                            // карточка `fact`, только заголовок и подробность
                            // берутся у `gui::publication_detail`, а не у
                            // технического текста операции.
                            if o.id == "host.publication" {
                                fact(o.dot, gui::PUBLICATION_TITLE.into(), gui::publication_detail(&cfg))
                            } else {
                                fact(o.dot, o.title.clone(), o.detail.clone())
                            }
                        })))
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

        let foot_actions: AnyElement = match tab {
            gui::Tab::Params => {
                let allowed = can_save(dirty, busy_now, has_progress);
                // Кнопка гаснет сама, но молча — заказчик 2026-10-20: почему
                // нельзя нажать, не видно. Причина теперь живёт в левом слоте
                // подвала (`foot_left`, один текст — заказчик 2026-10-20: «две
                // надписи стало») и в тултипе кнопки, вторая надпись рядом не
                // добавляется.
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("save")
                            .primary()
                            .label("Сохранить")
                            .disabled(!allowed)
                            .when_some(hint.clone(), |b, h| b.tooltip(h))
                            .on_click(cx.listener(|panel, _, window, cx| {
                                // Смена базы при включённых MCP спрашивает
                                // подтверждение: агент работает прямо сейчас и
                                // после записи перейдёт на другую базу. Во всех
                                // прочих случаях вопроса нет и запись идёт сразу.
                                // Перезапуск сеанса и строка в журнал — внутри
                                // `gui::save_host`, не здесь.
                                let host = panel.host_from_fields(cx);
                                let to_write = host.clone();
                                gui::confirm_database_change(window, cx, &host, move |_window, cx| {
                                    let ui = cx.global::<Ui>();
                                    let (Some(w), Some(panel)) = (ui.window, ui.panel.clone()) else { return };
                                    let _ = w.update(cx, |_, _window, cx| {
                                        panel.update(cx, |this, cx| {
                                            this.write(to_write, cx);
                                        });
                                    });
                                    cx.refresh_windows();
                                });
                            })),
                    )
                    .into_any_element()
            }
            // Главная кнопка ведёт человека по порядку: сначала поставить
            // расширение, потом — выгружать в git. `Busy::Ext` и
            // `Busy::Project` не различают, что именно идёт: на время
            // работы заблокировано всё равно всё.
            gui::Tab::Project => {
                let which = gui::project_primary(&cx.global::<Ui>().ops);
                let (label, hint) = match which {
                    Which::Export => ("Выгрузить", "Выгрузить из 1С"),
                    _ => ("Установить", "Установить расширение"),
                };
                // В меню — всё, кроме того, что на главной кнопке.
                let rest: Vec<(&'static str, Which)> = [
                    ("Выгрузить из 1С", Which::Export),
                    ("Загрузить в 1С", Which::Import),
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
                    // «Открыть отчёт last-check.txt» убран (заказчик
                    // 2026-10-20: зачем два пункта) — файл остаётся, на него
                    // называет сама строка в журнале после проверки
                    // (`Ev::Checks` в `mod.rs`).
                    menu.item(PopupMenuItem::new("Открыть журнал").disabled(busy_now).on_click(|_, _, cx| {
                        let path = cx.global::<Ui>().kit.state_path(gui::LOG_FILE);
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
                // `overflow_hidden`: бегунок едет от −25 % до 100 % ширины и
                // без обрезки высовывался бы за концы линии в поля окна.
                div().w_full().h(px(2.)).relative().overflow_hidden().bg(border.opacity(0.6)).child(
                    div()
                        .absolute()
                        .top_0()
                        .h(px(2.))
                        // Как у установщика (app.css `.progress`): бегунок 40 %,
                        // 1.4 с, плавный разгон и торможение — обе программы
                        // дышат одинаково (заказчик 2026-10-19).
                        .w(relative(0.4))
                        .rounded_full()
                        .bg(accent)
                        .with_animation(
                            "progress-thumb",
                            Animation::new(Duration::from_millis(1400)).repeat().with_easing(ease_in_out),
                            |d, t| d.left(relative(-0.4 + 1.4 * t)),
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
                    // Один текст, одна строка (заказчик 2026-10-20: перенос
                    // на вторую строку и подпрыгивание высоты подвала от
                    // длинного пути комплекта, кириллица в нём в том числе).
                    // Лишнее режется многоточием, а не переносится; `min_w_0`
                    // — иначе flex-ребёнок не уже своего текста, и обрезка на
                    // узком окне срабатывает только упёршись в кнопку.
                    // Полный текст — подсказкой по наведению.
                    .child({
                        let text = foot_left.0.clone();
                        div()
                            .id("foot-left")
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .text_color(if foot_left.1 { danger } else { caption })
                            .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
                            .child(foot_left.0)
                    })
                    .child(foot_actions),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{can_save, card, fields_differ, foot_left_text, project_rows, save_hint, shown_fields, HINT_PROJECT};
    use crate::cfg::Host;
    use crate::gui::{Dot, Kind, Line, ProjectView, Readiness, Running, Which};
    use crate::session::State;
    use common::check::Check;
    use common::Phase;
    use std::time::{Duration, Instant};

    #[test]
    fn the_card_follows_the_running_operation() {
        // Anchored an hour into the future, same as the helpers in
        // `gui::mod` tests: a fresh CI runner can have under 65s of
        // uptime, and `Instant::now() - Duration::from_secs(65)` panics.
        let now = Instant::now() + Duration::from_secs(3600);
        let r = Running {
            which: Which::Export,
            started: now - Duration::from_secs(65),
            phase: Phase::Push,
            seen: now,
            stale_noted: false,
        };
        let c = card(Some(&r), &State::Off, "tunnel", &Readiness::Unknown, now);
        assert_eq!(c.dot, Dot::Busy);
        assert_eq!(c.title, "Отправляю файлы · 1:05");
        assert!(c.cancel);
    }

    #[test]
    fn the_card_hides_cancel_for_the_platform() {
        // `cf_decompile`/`cf_load` на стороне платформы не прерываются —
        // «Прервать» не показывается, в отличие от git-обмена выше.
        let now = Instant::now() + Duration::from_secs(3600);
        let r = Running { which: Which::Export, started: now, phase: Phase::Decompile, seen: now, stale_noted: false };
        let c = card(Some(&r), &State::Off, "tunnel", &Readiness::Unknown, now);
        assert_eq!(c.dot, Dot::Busy);
        assert!(!c.cancel);
    }

    #[test]
    fn without_an_operation_the_card_is_the_link() {
        let now = Instant::now();
        let c = card(None, &State::Off, "tunnel", &Readiness::Unknown, now);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Off, "Не подключён"));
        assert!(!c.cancel);
        let c = card(None, &State::Connecting, "tunnel", &Readiness::Unknown, now);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Busy, "Подключаюсь…"));
        let c = card(None, &State::Up { since: Instant::now() }, "tunnel", &Readiness::Unknown, now);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Up, "Подключён"));
        let c = card(None, &State::Retrying("нет сети".into()), "tunnel", &Readiness::Unknown, now);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Busy, "Переподключаюсь"));
        let c = card(None, &State::Failed("нет сети".into()), "tunnel", &Readiness::Unknown, now);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Down, "Не подключён"));
        assert!(!c.cancel);
    }

    #[test]
    fn card_titles_speak_about_mcp_not_the_tunnel() {
        let now = Instant::now();
        assert_eq!(card(None, &State::Off, "local", &Readiness::Unknown, now).title, "MCP выключены");
        assert_eq!(card(None, &State::Connecting, "local", &Readiness::Unknown, now).title, "Включаю…");
        assert_eq!(card(None, &State::Up { since: now }, "local", &Readiness::Unknown, now).title, "MCP включены");
        // Отказ — причина, не общее «не подключён»: порт занят независимо
        // от переключателя, человеку нужно знать, чем именно.
        let c = card(None, &State::Failed("порт 7771 занят процессом 1234".into()), "local", &Readiness::Unknown, now);
        assert_eq!(c.title, "порт 7771 занят процессом 1234");
        for mode in ["local", "tunnel"] {
            for link in [
                State::Off,
                State::Connecting,
                State::Up { since: now },
                State::Retrying("нет сети".into()),
                State::Failed("порт занят".into()),
            ] {
                let title = card(None, &link, mode, &Readiness::Unknown, now).title;
                assert!(!title.to_lowercase().contains("туннель"), "{mode}/{link:?}: {title}");
            }
        }
    }

    /// Дефект (заказчик 2026-10-20, трижды в один день): зелёная карточка
    /// обещала «агент дойдёт до базы», хотя инструменты не разложены — первый
    /// провалившийся факт готовности (`Readiness::Warn`) должен перекрасить
    /// карточку в жёлтый и назвать причину, не трогая смысл самого
    /// переключателя (`Switch` в `render` не зависит от `ready`).
    #[test]
    fn a_failed_readiness_fact_turns_the_green_card_into_a_named_warning() {
        let now = Instant::now();
        let fail = Check::fail("инструменты комплекта", "в C:\\dev1c ничего нет", "1c-connect install");
        let ready = Readiness::Warn(fail);
        let c = card(None, &State::Up { since: now }, "local", &ready, now);
        assert_eq!(c.dot, Dot::Warn);
        assert!(c.title.starts_with("Агент базу не увидит:"), "{}", c.title);
        assert!(c.title.contains("инструменты комплекта") && c.title.contains("1c-connect install"), "{}", c.title);
        assert!(!c.title.contains("факт"), "{}", c.title);
    }

    /// Неизвестно (поток ещё считает) или пройдено — карточка остаётся
    /// зелёной, как раньше; «ещё не посчитано» не должно выглядеть как
    /// провал.
    #[test]
    fn unknown_or_passed_readiness_keeps_the_green_card() {
        let now = Instant::now();
        for ready in [Readiness::Unknown, Readiness::Ok] {
            let c = card(None, &State::Up { since: now }, "local", &ready, now);
            assert_eq!((c.dot, c.title.as_str()), (Dot::Up, "MCP включены"), "{ready:?}");
        }
    }

    /// Туннельный режим не говорит «MCP» и не смотрит готовность — его
    /// собственный провал публикации уже красный (`State::Failed`) отдельным
    /// путём (`Ev::Publish` в `gui::mod`).
    #[test]
    fn tunnel_mode_ignores_the_readiness_warning() {
        let now = Instant::now();
        let fail = Check::fail("инструменты комплекта", "в C:\\dev1c ничего нет", "1c-connect install");
        let c = card(None, &State::Up { since: now }, "tunnel", &Readiness::Warn(fail), now);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Up, "Подключён"));
    }

    #[test]
    fn dirty_ignores_dump_tools_and_base_url_but_sees_a_changed_field() {
        let saved = Host { dump: "C:\\proj\\src".into(), tools: vec!["x".into()], ..Host::default() };
        // Поля «Адрес публикации» в окне нет — `host_of` отдаёт его пустым.
        let shown = Host { dump: String::new(), tools: Vec::new(), base_url: String::new(), ..saved.clone() };
        assert!(!fields_differ(&shown, &saved, "tunnel"));
        let edited = Host { server: "h:1".into(), ..shown.clone() };
        assert!(fields_differ(&edited, &saved, "tunnel"));
        // Каталог проекта теперь в форме — его правка видна.
        let moved = Host { project: "D:\\other\\Buh".into(), ..shown };
        assert!(fields_differ(&moved, &saved, "tunnel"));
    }

    #[test]
    fn local_mode_hides_the_environment_fields() {
        assert_eq!(shown_fields("local").to_vec(), vec!["base_path", "db_user", "db_password", "project"]);
        assert_eq!(shown_fields("tunnel").len(), 7);
        // Что угодно, кроме "local", — тот же (полный) список, что у "tunnel".
        assert_eq!(shown_fields("bogus"), shown_fields("tunnel"));
    }

    #[test]
    fn changing_a_hidden_field_does_not_enable_save() {
        let saved = Host { mode: "local".into(), ..Host::default() };
        // Адрес среды, пароль рабочего места и само рабочее место правятся,
        // но в локальном режиме ни на что не влияют — «Сохранить» молчит.
        let edited = Host {
            server: "10.0.0.5:3393".into(),
            password: "другой".into(),
            workstation: "dev2".into(),
            ..saved.clone()
        };
        assert!(!fields_differ(&edited, &saved, "local"));
        // А то же самое в туннельном режиме — заметная правка.
        assert!(fields_differ(&edited, &saved, "tunnel"));
        // Видимое поле в локальном режиме по-прежнему включает «Сохранить».
        let moved = Host { project: "D:\\other\\Buh".into(), ..saved.clone() };
        assert!(fields_differ(&moved, &saved, "local"));
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

    #[test]
    fn save_is_blocked_while_an_operation_runs() {
        assert!(can_save(true, false, false));
        assert!(!can_save(false, false, false), "ничего не менялось");
        assert!(!can_save(true, true, false), "идёт операция/проверка");
        assert!(!can_save(true, false, true), "сеанс подключается/перезапускается");
    }

    #[test]
    fn save_hint_names_the_reason_it_is_disabled() {
        // Можно нажать — объяснять нечего.
        assert_eq!(save_hint(true, false, None), None);
        // Нечего сохранять: поля не трогали.
        assert_eq!(save_hint(false, false, None).as_deref(), Some("Нечего сохранять — поля ещё не менялись"));
        // Идёт операция — названная, а не общим словом.
        assert_eq!(save_hint(true, true, Some(Which::Export)).as_deref(), Some("Подождите: идёт выгрузка из 1С"));
        assert_eq!(save_hint(true, true, Some(Which::Import)).as_deref(), Some("Подождите: идёт загрузка в 1С"));
        assert_eq!(save_hint(true, true, Some(Which::Install)).as_deref(), Some("Подождите: идёт работа с расширением"));
        assert_eq!(save_hint(true, true, Some(Which::Uninstall)).as_deref(), Some("Подождите: идёт работа с расширением"));
        // Занято, но никакая операция не идёт — значит идёт проверка.
        assert_eq!(save_hint(true, true, None).as_deref(), Some("Подождите: идёт проверка"));
    }

    #[test]
    fn the_footer_slot_picks_one_text_in_priority_order() {
        let path = "C:\\Users\\имя\\AppData\\Local\\dev1c";
        // Сказать нечего — путь комплекта, кнопка не красная.
        assert_eq!(foot_left_text(None, None, path), (path.to_string(), false));
        // Подсказка, почему «Сохранить» недоступна, — вместо пути, а не рядом с ним.
        assert_eq!(foot_left_text(None, Some("Нечего сохранять"), path), ("Нечего сохранять".to_string(), false));
        // Настоящий итог сильнее подсказки — человек не теряет ошибку, потому
        // что слот уже занят объяснением (заказчик 2026-10-20).
        let ok = Line { kind: Kind::Ok, text: "Сохранено в 1c-connect.toml".into() };
        assert_eq!(foot_left_text(Some(&ok), Some("Нечего сохранять"), path), (ok.text.clone(), false));
        let err = Line { kind: Kind::Fail, text: "порт занят".into() };
        assert_eq!(foot_left_text(Some(&err), Some("Нечего сохранять"), path), (err.text.clone(), true));
    }
}
