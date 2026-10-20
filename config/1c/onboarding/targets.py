# Прицел для онбординга: чистая логика «куда показывать» — без gi/GTK,
# чтобы модуль можно было проверить и на машине без дисплея. Координаты
# берутся из реальной геометрии (раскладка xfdesktop, список окон,
# рабочая область панели); единственные зашитые пиксели — калибровка
# сетки значков и области трея, вынесенные в константы ниже.
import re
from collections import namedtuple
from typing import Callable

Rect = namedtuple('Rect', 'x y w h')
WindowInfo = namedtuple('WindowInfo', 'name wclass rect stacking')

# Калибровка сетки значков рабочего стола xfdesktop: центр значка (0,0) —
# (60, 40), шаг 106px по обеим осям при icon_size=48. Замерено по снимку
# рабочего стола стенда. Первое, что правят, если стрелка промахнётся.
ICON_GRID_ORIGIN_X = 60
ICON_GRID_ORIGIN_Y = 40
ICON_GRID_STEP = 106

# Область трея на нижней панели. У значка трея нет своей геометрии в X11,
# которую можно было бы запросить, — только калибровка, как и у сетки
# значков выше.
#
# Правее трея на панели стенда стоят раскладка клавиатуры, часы и значок
# сеанса; вместе они занимают около 440 пикселей. Без этой поправки
# стрелка показывала на часы, а не на коннектор: замечено на стенде
# 20.10.2026, значок коннектора был примерно в 500 пикселях от правого
# края экрана, а область трея кончалась в 160.
TRAY_RIGHT_MARGIN = 440
TRAY_WIDTH = 120
TRAY_HEIGHT = 40

# Запись раскладки xfdesktop (~/.config/xfce4/desktop/icons.screen0.yaml):
# ключ — абсолютный путь в кавычках, значение — отображение row/col:
#   "/home/dev1/Desktop/foo.desktop":
#     row: N
#     col: N
# Файл называется .yaml, но парсим его регуляркой, а не YAML-библиотекой —
# в стандартной библиотеке её нет, а своих записей всего три поля.
_ICON_ENTRY_RE = re.compile(r'"(?P<path>[^"]+)":\s*row:\s*(?P<row>\d+)\s*col:\s*(?P<col>\d+)')


def desktop_icon(layout_text: str, desktop_file: str, exists: Callable[[str], bool], icon_size: int = 48) -> Rect | None:
    """Прямоугольник значка desktop_file по раскладке xfdesktop.

    Раскладка помнит и значки, которых уже нет на диске (устаревшие
    записи) — такая запись игнорируется, даже если формально нашлась.
    """
    for m in _ICON_ENTRY_RE.finditer(layout_text):
        path = m.group('path')
        if not path.endswith(desktop_file):
            continue
        if not exists(path):
            return None
        row = int(m.group('row'))
        col = int(m.group('col'))
        cx = ICON_GRID_ORIGIN_X + col * ICON_GRID_STEP
        cy = ICON_GRID_ORIGIN_Y + row * ICON_GRID_STEP
        half = icon_size // 2
        return Rect(cx - half, cy - half, icon_size, icon_size)
    return None


def window(lister: Callable[[], list[WindowInfo]], name_pattern: str) -> Rect | None:
    """Прямоугольник окна, чьё имя ИЛИ класс содержит name_pattern.

    Без учёта регистра. Если подходит несколько окон — берётся верхнее
    по stacking (самое видимое).

    Класс смотрим наравне с именем, потому что заголовок окна редактора
    принадлежит не редактору, а открытому в нём проекту: на стенде Zed
    показывает «proj — SKILL.md», слова «Zed» в заголовке нет вовсе, и
    шаги 4 и 5 оставались без стрелки. Класс окна (dev.zed.Zed) от
    содержимого не зависит. Найдено глазами на стенде 20.10.2026.
    """
    pattern = name_pattern.lower()
    matches = [w for w in lister()
               if pattern in w.name.lower() or pattern in w.wclass.lower()]
    if not matches:
        return None
    return max(matches, key=lambda w: w.stacking).rect


# Место, которое занимает окно коннектора, когда его открывают. Оно само
# прижимается к правому нижнему углу рабочей области с отступом
# (gui/window.rs: flyout_origin, GAP = 12) и не бывает меньше 380x420
# (MIN_SIZE); по содержимому может стать выше, поэтому резервируем с
# запасом по высоте.
#
# Нужно шагам про коннектор: они предлагают человеку открыть окно, и
# выноска обязана оставить ему место заранее. Иначе окно открывается
# ровно под выноской — и шаг накрывает собой то, о чём говорит (найдено
# глазами на стенде 20.10.2026).
CONNECTOR_WIDTH = 380
CONNECTOR_HEIGHT = 560
CONNECTOR_GAP = 12


def connector_area(workarea: Rect) -> Rect:
    """Прямоугольник, который займёт окно коннектора в правом нижнем углу."""
    x = workarea.x + workarea.w - CONNECTOR_WIDTH - CONNECTOR_GAP
    y = workarea.y + workarea.h - CONNECTOR_HEIGHT - CONNECTOR_GAP
    return Rect(max(x, workarea.x), max(y, workarea.y),
                min(CONNECTOR_WIDTH, workarea.w), min(CONNECTOR_HEIGHT, workarea.h))


def tray_area(workarea: Rect) -> Rect:
    """Область трея у правого края нижней панели, внутри workarea."""
    x = workarea.x + workarea.w - TRAY_RIGHT_MARGIN - TRAY_WIDTH
    y = workarea.y + workarea.h - TRAY_HEIGHT
    return Rect(x, y, TRAY_WIDTH, TRAY_HEIGHT)


# Общий список для anchor_in и для scenario.py (проверка на этапе
# загрузки TOML — опечатка в anchor обязана упасть при load(), а не
# когда-нибудь потом внутри World/Machine на живом стенде).
ANCHOR_KINDS = ('whole', 'left', 'middle', 'right', 'right-bottom')


def anchor_in(rect: Rect, anchor: str) -> Rect:
    """Сужает rect до его части.

    'whole' — весь прямоугольник. 'right' — правая треть, по всей высоте.
    'right-bottom' — правая треть, нижняя четверть: нужно для панели
    агента Zed, у которой нет своего окна — только доля от окна целиком.
    'left' — левая треть, по всей высоте.
    'middle' — средняя треть: дерево файлов проекта в Zed. Именно средняя,
    а не левая: у редактора на стенде три колонки, и слева список бесед
    агента, а не файлы (проверено на живом окне 2026-10-20).
    """
    if anchor == 'whole':
        return rect
    third = rect.w // 3
    if anchor == 'left':
        return Rect(rect.x, rect.y, third, rect.h)
    if anchor == 'middle':
        return Rect(rect.x + third, rect.y, third, rect.h)
    x = rect.x + rect.w - third
    if anchor == 'right':
        return Rect(x, rect.y, third, rect.h)
    if anchor == 'right-bottom':
        quarter = rect.h // 4
        y = rect.y + rect.h - quarter
        return Rect(x, y, third, quarter)
    raise ValueError(f'неизвестный anchor: {anchor!r}')
