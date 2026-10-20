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

_STACKING_RE = re.compile(r'#\s*(?P<ids>.+)$')
_NAME_RE = re.compile(r'=\s*"(?P<name>(?:[^"\\]|\\.)*)"')
_GEOM_RE = {
    'x': re.compile(r'Absolute upper-left X:\s*(-?\d+)'),
    'y': re.compile(r'Absolute upper-left Y:\s*(-?\d+)'),
    'w': re.compile(r'Width:\s*(\d+)'),
    'h': re.compile(r'Height:\s*(\d+)'),
}


def _run(args: list[str]) -> str:
    """Тихий subprocess с коротким таймаутом. Окно может исчезнуть между
    получением списка и опросом — это нормальный ход вещей, не авария."""
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=2).stdout
    except (OSError, subprocess.SubprocessError):
        return ''


def _window_name(wid: str) -> str | None:
    out = _run(['xprop', '-id', wid, '_NET_WM_NAME', 'WM_NAME'])
    m = _NAME_RE.search(out)
    return None if m is None else m.group('name').replace('\\"', '"')


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
    совпадение нескольких окон решается по максимальному stacking)."""
    out = _run(['xprop', '-root', '_NET_CLIENT_LIST_STACKING'])
    m = _STACKING_RE.search(out)
    if m is None:
        return []
    windows = []
    for stacking, wid in enumerate(i.strip() for i in m.group('ids').split(',') if i.strip()):
        name = _window_name(wid)
        rect = _window_geometry(wid)
        if name is None or rect is None:
            continue
        windows.append(WindowInfo(name, '', rect, stacking))
    return windows


def _overlaps(a: Rect, b: Rect) -> bool:
    return not (a.x + a.w <= b.x or b.x + b.w <= a.x or a.y + a.h <= b.y or b.y + b.h <= a.y)


class World:
    """Мир онбординга поверх настоящего рабочего стола."""

    def icon(self, desktop_file: str) -> Rect | None:
        try:
            layout_text = _LAYOUT_PATH.read_text(encoding='utf-8')
        except OSError:
            return None  # xfdesktop ещё не написал раскладку
        return desktop_icon(layout_text, desktop_file, exists=lambda p: pathlib.Path(p).exists())

    def window(self, pattern: str, anchor: str = 'whole') -> Rect | None:
        rect = _window_rect(_list_windows, pattern)
        return None if rect is None else anchor_in(rect, anchor)

    def covered(self, rect: Rect) -> bool:
        """rect закрыт, если что-то выше его в стеке с ним пересекается.

        Для значка и области трея (у них своего окна нет) выше — любое
        окно; для прямоугольника настоящего окна (пришёл из window())
        окно с тем же прямоугольником — не чужое, а оно само, и не
        считается перекрывающим себя."""
        windows = _list_windows()
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
