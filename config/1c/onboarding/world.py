# Настоящий World онбординга: то, что задача 5 подделывала в FakeWorld —
# через живой X11 и GTK, без выдуманных чисел.
#
# Список окон читается через xprop/xwininfo, а не через Wnck/wmctrl/xdotool:
# ни gir1.2-wnck-3.0, ни wmctrl/xdotool в образ не добавлены (список
# пакетов в брифе задачи фиксирован — python3-gi, python3-gi-cairo,
# gir1.2-gtk-3.0), а xprop и xwininfo там уже стоят (x11-utils, нужны по
# другим причинам). Gdk.Window.property_get в PyGObject не работает
# (AttributeError), Gdk.Screen.get_window_stack() даёт геометрию, но не
# имя — поэтому для окон используется только то, что реально работает:
# две команды, а не смесь двух несовместимых API.
#
# Площадь экрана и рабочей области панели — наоборот, честные запросы к
# Gdk.Screen: тут GTK знает всё сам, скрипт тут не нужен.
import pathlib
import re
import subprocess

import gi

gi.require_version('Gdk', '3.0')
from gi.repository import Gdk

from targets import Rect, WindowInfo, anchor_in, desktop_icon
from targets import window as _window_rect

_LAYOUT_PATH = pathlib.Path.home() / '.config' / 'xfce4' / 'desktop' / 'icons.screen0.yaml'

# Свойство ищем по имени, не первой попавшейся кавычкой в блоке: один
# xprop-вызов ниже спрашивает сразу имя и класс, и если взять «первую
# строку в кавычках» без привязки к имени свойства, можно случайно
# прочесть WM_CLASS вместо _NET_WM_NAME (или наоборот), если xprop
# когда-нибудь поменяет порядок вывода.
def _prop(out: str, name: str) -> re.Match | None:
    return re.search(re.escape(name) + r'\([^)]*\)\s*=\s*"(?P<v>(?:[^"\\]|\\.)*)"', out)


_STACKING_RE = re.compile(r'#\s*(?P<ids>.+)$')
_GEOM_RE = {
    'x': re.compile(r'Absolute upper-left X:\s*(-?\d+)'),
    'y': re.compile(r'Absolute upper-left Y:\s*(-?\d+)'),
    'w': re.compile(r'Width:\s*(\d+)'),
    'h': re.compile(r'Height:\s*(\d+)'),
}

# Классы окон, которые не считаются «чем-то поверх цели» — это не
# программа, с которой человек взаимодействует, а часть самого стола:
#
#   xfdesktop    — окно рабочего стола во весь экран, всегда существует.
#                  У значка (в отличие от настоящего окна) нет своего
#                  stacking, чтобы отличить «подо мной» от «надо мной» —
#                  см. own_stacking в covered() ниже, — поэтому без
#                  исключения xfdesktop перекрывал бы вообще любой
#                  значок и любую область трея на любом живом стенде
#                  (ровно это и увидел координатор: covered=True везде).
#   xfce4-panel  — нижняя панель. По stacking её от обычных окон не
#                  отличить принципиально: xfwm4 волен держать панель
#                  поверх окон («always on top») или под ними, это
#                  вопрос настройки, а не гарантия — на живом стенде
#                  видели оба случая. Значит единственный надёжный
#                  признак — класс, тот же приём, что и для стола.
#
# Оба имени — то, что реально отдаёт WM_CLASS на живом стенде dev1c
# (первый из двух компонентов; второй — то же самое с заглавной буквы,
# "Xfdesktop"/"Xfce4-panel"): проверено и на запасном Xvfb-стенде, и на
# настоящей RDP-сессии координатора (xprop -id <id> WM_CLASS, 2026-10-20).
#
# Собственная выноска (callout.Callout) сюда не входит и не нуждается во
# включении: это Gtk.WindowType.POPUP (override-redirect), а такие окна
# оконный менеджер вообще не ведёт — список _NET_CLIENT_LIST_STACKING до
# показа выноски и после неё идентичен (проверено: запустил Callout под
# Xvfb+xfwm4 и сравнил xprop-слепок списка окон до/после — ни одной новой
# записи). Будь выноска обычным Gtk.Window (TOPLEVEL), она бы туда
# попадала, и этот список понадобилось бы пополнить её классом тоже.
_NOT_COVERING_WCLASSES = {'xfdesktop', 'xfce4-panel'}


def _run(args: list[str]) -> str:
    """Тихий subprocess с коротким таймаутом. Окно может исчезнуть между
    получением списка и опросом — это нормальный ход вещей, не авария."""
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=2).stdout
    except (OSError, subprocess.SubprocessError):
        return ''


def _window_name_and_class(wid: str) -> tuple[str | None, str]:
    # Одним вызовом: имя и класс почти всегда нужны вместе (_list_windows
    # строит WindowInfo целиком), и это вдвое меньше subprocess на окно.
    out = _run(['xprop', '-id', wid, '_NET_WM_NAME', 'WM_NAME', 'WM_CLASS'])
    name_m = _prop(out, '_NET_WM_NAME') or _prop(out, 'WM_NAME')
    name = None if name_m is None else name_m.group('v').replace('\\"', '"')
    # WM_CLASS несёт два значения через запятую — instance, class
    # ("xfdesktop", "Xfdesktop"); _prop находит первую кавычку после
    # имени свойства, то есть ровно instance — то же, что видно в живом
    # xprop-слепке и что назвал координатор.
    class_m = _prop(out, 'WM_CLASS')
    wclass = class_m.group('v') if class_m else ''
    return name, wclass


def _window_geometry(wid: str) -> Rect | None:
    out = _run(['xwininfo', '-id', wid])
    values = {}
    for key, pattern in _GEOM_RE.items():
        m = pattern.search(out)
        if m is None:
            return None
        values[key] = int(m.group(1))
    return Rect(values['x'], values['y'], values['w'], values['h'])


def _list_windows() -> list[WindowInfo]:
    """Окна в порядке стека — младший индекс снизу, как и держит стек
    оконного менеджера (та же договорённость, что у targets.window:
    совпадение нескольких окон решается по максимальному stacking).
    Несёт и xfdesktop, и панель — их исключает covered(), не этот
    список: window() ищет окно по имени среди них же (ни тому, ни
    другому «Zed» или «1cv8» не подходит), фильтровать на входе незачем."""
    out = _run(['xprop', '-root', '_NET_CLIENT_LIST_STACKING'])
    m = _STACKING_RE.search(out)
    if m is None:
        return []
    windows = []
    for stacking, wid in enumerate(i.strip() for i in m.group('ids').split(',') if i.strip()):
        name, wclass = _window_name_and_class(wid)
        rect = _window_geometry(wid)
        if name is None or rect is None:
            continue
        windows.append(WindowInfo(name, wclass, rect, stacking))
    return windows


def _overlaps(a: Rect, b: Rect) -> bool:
    return not (a.x + a.w <= b.x or b.x + b.w <= a.x or a.y + a.h <= b.y or b.y + b.h <= a.y)


class World:
    """Мир онбординга поверх настоящего рабочего стола.

    window_lister — источник списка окон, по умолчанию настоящий
    _list_windows (xprop/xwininfo). Подменяется в тестах готовым
    списком WindowInfo, чтобы проверить covered()/window() без живого
    X11 — тем же World, с той же логикой исключения стола и панели, а
    не отдельным дублирующим её фейком (см. tests/test_world.py)."""

    def __init__(self, window_lister=_list_windows):
        self._window_lister = window_lister

    def icon(self, desktop_file: str) -> Rect | None:
        try:
            layout_text = _LAYOUT_PATH.read_text(encoding='utf-8')
        except OSError:
            return None  # xfdesktop ещё не написал раскладку
        return desktop_icon(layout_text, desktop_file, exists=lambda p: pathlib.Path(p).exists())

    def window(self, pattern: str, anchor: str = 'whole') -> Rect | None:
        rect = _window_rect(self._window_lister, pattern)
        return None if rect is None else anchor_in(rect, anchor)

    def covered(self, rect: Rect) -> bool:
        """rect закрыт, если что-то выше его в стеке с ним пересекается.

        Для значка и области трея (у них своего окна нет) выше — любое
        окно, кроме стола и панели (_NOT_COVERING_WCLASSES — без этого
        они перекрывали бы вообще любой значок и любую цель вообще,
        потому что own_stacking ниже не из чего сравнивать: у значка
        нет собственного места в стеке, см. комментарий у
        _NOT_COVERING_WCLASSES). Для прямоугольника настоящего окна
        (пришёл из window()) окно с тем же прямоугольником — не чужое, а
        оно само, и не считается перекрывающим себя."""
        windows = [w for w in self._window_lister() if w.wclass not in _NOT_COVERING_WCLASSES]
        own_stacking = next((w.stacking for w in windows if w.rect == rect), None)
        for w in windows:
            if w.rect == rect:
                continue
            if own_stacking is not None and w.stacking <= own_stacking:
                continue
            if _overlaps(w.rect, rect):
                return True
        return False

    def screen(self) -> Rect:
        """Рабочая область основного монитора (без панели) — тем же
        прямоугольником и для размещения выноски, и для области трея."""
        screen = Gdk.Screen.get_default()
        area = screen.get_monitor_workarea(0)
        return Rect(area.x, area.y, area.width, area.height)
