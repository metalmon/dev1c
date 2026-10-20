# Тест на форму окна выноски: окно должно быть вырезано по контуру
# пузыря и стрелки, а не залито прямоугольником целиком. Проверяется
# через реальный GTK и Xvfb — геометрия маски формы, которую строит
# X-сервер, иначе не проверить.
#
# Сама обрезка формы проверяется запросом к окну (xwininfo -shape,
# x11-utils, уже в образе), не чтением пикселя фона: координатор поймал
# это нестабильным в чистом контейнере без рабочего стола — заливка
# корневого окна через отдельное соединение Gdk.Display.open() и снимок
# экрана через другое такое же соединение ничем не гарантированно
# согласованы друг с другом (ни sync, ни то, что это вообще один и тот
# же X-сервер между вызовами), и в контейнере без предыдущих прогонов,
# оставивших в корневом окне что-то отличное от чёрного, тест падал. То,
# что окно обрезано, — факт самого окна (форма, которую применил
# X-сервер), а не следствие, которое нужно подглядывать в пикселе фона
# за его пределами. Снимок экрана (grab) остался — но только для точек
# ВНУТРИ самого пузыря (своя же заливка, цвет кнопки и т.п.), это
# рисует этот же процесс в этом же кадре и ни от чего постороннего не
# зависит.
import os
import pathlib
import re
import subprocess
import sys
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk, Gtk

from scenario import Step, Target
from targets import Rect
from placement import bubble_size, place
from callout import HEAD, LINE_HEIGHT, Callout

DISPLAY = os.environ.get('ONBOARDING_TEST_DISPLAY', ':77')
WHITE = (255, 255, 255)
BUTTON_AMBER = (0xE6, 0xB8, 0x00)

SCREEN = Rect(0, 0, 1280, 800)
_TARGET = Target('none', None, None, 'whole')

_SHAPE_EXTENTS_RE = re.compile(r'Window shape extents:\s*(\d+)x(\d+)\+(\d+)\+(\d+)')


def pump_gtk(seconds):
    """Прокачивает цикл GTK заданное время, чтобы окно успело отрисоваться."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        while Gtk.events_pending():
            Gtk.main_iteration()
        time.sleep(0.01)


def _root_window(display):
    return Gdk.Display.open(display).get_default_screen().get_root_window()


def shape_extents(display, xid):
    """Прямоугольник (локальные координаты окна), на который X-сервер
    реально обрезал окно формой — `None`, если формы вовсе нет (окно
    прямоугольное). Спрашивает у самого окна (xwininfo -shape), не
    выводит из пикселя за его пределами."""
    env = {**os.environ, 'DISPLAY': display}
    out = subprocess.run(['xwininfo', '-shape', '-id', hex(xid)],
                          capture_output=True, text=True, env=env, check=True).stdout
    m = _SHAPE_EXTENTS_RE.search(out)
    if m is None:
        return None
    w, h, x, y = (int(v) for v in m.groups())
    return Rect(x, y, w, h)


class _Shot:
    """Снимок экрана: чтение пикселя по абсолютным координатам, из
    сырых байтов GdkPixbuf (RGB, без альфы — у корневого окна её нет)."""

    def __init__(self, pixbuf):
        self._pixels = pixbuf.get_pixels()
        self._stride = pixbuf.get_rowstride()
        self._channels = pixbuf.get_n_channels()

    def pixel(self, x, y):
        offset = y * self._stride + x * self._channels
        return tuple(self._pixels[offset:offset + 3])


def grab(display):
    """Снимок экрана заданного дисплея через Gdk.pixbuf_get_from_window."""
    root = _root_window(display)
    pixbuf = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
    return _Shot(pixbuf)


def _step(body='Текст тела.', buttons=('Дальше',), questions=()):
    return Step(1, 'Заголовок', body, _TARGET, buttons, 'button', questions)


class _FakeEvent:
    """Хватает _on_click: ей нужны только x и y в локальных координатах
    окна — тех же, что несут _question_rects/_button_rects/_close_rect."""

    def __init__(self, x, y):
        self.x = x
        self.y = y


class ShapeTests(unittest.TestCase):
    def setUp(self):
        os.environ['DISPLAY'] = DISPLAY

    def test_window_is_shaped_not_rectangular(self):
        # Пустое тело — так между заголовком и строкой кнопок остаётся
        # заведомо пустая полоса: с текстом в одну строку буквы тела
        # почти упираются в кнопки, и внутренняя проба рисковала бы
        # задеть чернила, а не саму заливку.
        step = _step(body='')
        layout = place(SCREEN, None, bubble_size(step))
        callout = Callout(step, layout, on_button=lambda _: None)
        try:
            callout.show()
            pump_gtk(1.0)
            xid = callout.window.get_window().get_xid()
            shape = shape_extents(DISPLAY, xid)
            # Форма вообще есть (не None — не прямоугольное окно без
            # обрезки) и со всех четырёх сторон меньше самого окна —
            # значит углы окна обрезаны, а не залиты прямоугольником
            # целиком. Запрос к самому окну, не пиксель фона за ним.
            self.assertIsNotNone(shape)
            self.assertGreater(shape.x, 0)
            self.assertGreater(shape.y, 0)
            self.assertLess(shape.x + shape.w, callout._width)
            self.assertLess(shape.y + shape.h, callout._height)
            # Внутри самого пузыря — должна быть его белая заливка. Точка
            # выбрана ниже заголовка (его чернила заканчиваются примерно на
            # 48px от верха пузыря при пустом теле), выше строки кнопок
            # (она начинается на 64px от верха) и лишь немного правее левой
            # рамки — пустая полоса между текстом и кнопками, без шрифта.
            # Это пиксель содержимого самого окна (своя же заливка), не
            # фона за его пределами — от чужого состояния не зависит.
            img = grab(DISPLAY)
            self.assertEqual(img.pixel(layout.bubble.x + 12, layout.bubble.y + 58), WHITE)
        finally:
            callout.hide()

    def test_first_button_is_primary(self):
        # Первая кнопка в списке — главное действие (так в утверждённых
        # макетах на каждом экране), поэтому именно она должна быть
        # закрашена жёлтым; вторая — вторичная, без заливки (сквозь неё
        # видна белая заливка пузыря, а не жёлтый).
        step = _step(buttons=('Начать', 'Закрыть'))
        layout = place(SCREEN, None, bubble_size(step))
        callout = Callout(step, layout, on_button=lambda _: None)
        try:
            callout.show()
            pump_gtk(1.0)
            img = grab(DISPLAY)
            ox, oy = callout._origin
            rects = dict(callout._button_rects)
            primary = rects['Начать']
            secondary = rects['Закрыть']
            # Не геометрический центр: подпись начинается через 12px от
            # левого края кнопки, и её буквы попадают точно в середину —
            # так же, как чернила заголовка чуть не испортили пробу формы
            # выше. Берём точку у левого края, внутри рамки, но перед
            # первой буквой — там только заливка.
            def sample_point(rect):
                return (ox + rect.x + 6, oy + rect.y + rect.h // 2)
            self.assertEqual(img.pixel(*sample_point(primary)), BUTTON_AMBER)
            self.assertEqual(img.pixel(*sample_point(secondary)), WHITE)
        finally:
            callout.hide()


class WrapTests(unittest.TestCase):
    # Фикс-раунд 2: тело шага 1 — один длинный абзац без переносов (TOML
    # "\"-продолжение склеивает его в одну логическую строку). Без переноса
    # в placement.wrap_body эта строка рисовалась от x+16 на всю свою
    # ширину — далеко за правый край пузыря — и обрезалась маской формы
    # окна; именно это видно на скриншоте координатора.
    def test_long_body_text_does_not_run_past_the_right_margin(self):
        long_body = (
            'Здесь уже стоит учебная 1С с «Управлением нашей фирмой» и редактор, '
            'в котором живёт ИИ-агент. Ставить ничего не нужно, ломать тут нечего. '
            'Шесть коротких шагов, минуты три.'
        )
        step = _step(body=long_body)
        layout = place(SCREEN, None, bubble_size(step))
        callout = Callout(step, layout, on_button=lambda _: None)
        try:
            callout.show()
            pump_gtk(1.0)
            img = grab(DISPLAY)
            # Первая строка тела (i=0 в _draw_body), у правого внутреннего
            # края пузыря (20px от рамки — внутри неё, но не на самой
            # рамке, у которой тут же есть свой 3px чёрный штрих): до
            # фикса здесь были чернила неперенесённой строки (проверено
            # руками — на этом самом тексте, тем же сэмплом, без wrap_body
            # пиксель (87, 87, 87), не белый), после фикса — только заливка.
            sample_x = layout.bubble.x + layout.bubble.w - 20
            sample_y = layout.bubble.y + HEAD + 1 * LINE_HEIGHT - 6
            self.assertEqual(img.pixel(sample_x, sample_y), WHITE)
        finally:
            callout.hide()


class QuestionTests(unittest.TestCase):
    # Фикс-раунд 1, задача 6: шаг 4 без этого показывает три вопроса, на
    # которые нельзя нажать. Клик должен и вызвать copy_question, и
    # заметно пометить именно ту строку — без этого человек не узнает,
    # что клик вообще сработал.
    def test_clicking_a_question_copies_it_and_marks_it(self):
        text = 'Какая номенклатура есть в базе'
        step = _step(questions=(text, 'Другой вопрос'))
        layout = place(SCREEN, None, bubble_size(step))
        copied = []
        callout = Callout(step, layout, on_button=lambda _: None,
                           copy_question=lambda q: copied.append(q) or True)
        try:
            callout.show()
            pump_gtk(0.5)
            self.assertEqual([t for t, _ in callout._question_rects], [text, 'Другой вопрос'])
            clicked_text, rect = callout._question_rects[0]
            # Точка внутри заливки строки, левее текста (который начинается
            # на +8 от rect.x) — та же логика отступа, что у теста кнопок
            # ниже: подложку, а не чернила текста или тонкую 1px-рамку.
            sample = (callout._origin[0] + rect.x + 3, callout._origin[1] + rect.y + rect.h // 2)
            img_before = grab(DISPLAY)
            self.assertEqual(img_before.pixel(*sample), (0xF2, 0xF1, 0xEC))  # _QUESTION_BG

            callout._on_click(None, _FakeEvent(rect.x + 10, rect.y + rect.h // 2))

            self.assertEqual(copied, [text])
            self.assertEqual(clicked_text, text)
            self.assertEqual(callout._copied_question, text)
            pump_gtk(0.5)
            img_after = grab(DISPLAY)
            # Подложка строки сменилась на янтарный оттенок — заметная
            # отметка, что клик сработал, а не осталась прежней бледной.
            self.assertEqual(img_after.pixel(*sample), (0xF7, 0xE8, 0xB8))  # _QUESTION_BG_COPIED
        finally:
            callout.hide()

    def test_bubble_without_questions_draws_no_rows(self):
        step = _step()  # questions=() по умолчанию
        layout = place(SCREEN, None, bubble_size(step))
        callout = Callout(step, layout, on_button=lambda _: None)
        try:
            callout.show()
            pump_gtk(0.5)
            self.assertEqual(callout._question_rects, ())
        finally:
            callout.hide()


if __name__ == '__main__':
    unittest.main()
