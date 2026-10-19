# Тест на форму окна выноски: окно должно быть вырезано по контуру
# пузыря и стрелки, а не залито прямоугольником целиком. Проверяется
# через реальный GTK и Xvfb — геометрия маски формы, которую строит
# X-сервер, иначе не проверить.
import os
import pathlib
import subprocess
import sys
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk
from PIL import Image

from scenario import Step, Target
from targets import Rect
from placement import bubble_size, place
from callout import Callout

DISPLAY = os.environ.get('ONBOARDING_TEST_DISPLAY', ':77')
BLUE = (0, 0, 255)
WHITE = (255, 255, 255)

SCREEN = Rect(0, 0, 1280, 800)
_TARGET = Target('none', None, None, 'whole')
_SHOT_PATH = '/tmp/onboarding_test_shot.png'


def pump_gtk(seconds):
    """Прокачивает цикл GTK заданное время, чтобы окно успело отрисоваться."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        while Gtk.events_pending():
            Gtk.main_iteration()
        time.sleep(0.01)


def fill_root(display, color):
    """Заливает корневое окно известным цветом — фон, который виден там,
    где фигура окна вырезана."""
    r, g, b = color
    env = {**os.environ, 'DISPLAY': display}
    subprocess.run(['xsetroot', '-solid', '#%02x%02x%02x' % (r, g, b)],
                    env=env, check=True)


class _Shot:
    """Снимок экрана: чтение пикселя по абсолютным координатам."""

    def __init__(self, path):
        self._image = Image.open(path).convert('RGB')

    def pixel(self, x, y):
        return self._image.getpixel((x, y))


def grab(display):
    """Снимок экрана заданного дисплея через scrot."""
    env = {**os.environ, 'DISPLAY': display}
    subprocess.run(['scrot', '-o', _SHOT_PATH], env=env, check=True)
    return _Shot(_SHOT_PATH)


def _step():
    return Step(1, 'Заголовок', 'Текст тела.', _TARGET, ('Дальше',), 'button', ())


class ShapeTests(unittest.TestCase):
    def setUp(self):
        fill_root(DISPLAY, BLUE)
        os.environ['DISPLAY'] = DISPLAY

    def test_window_is_shaped_not_rectangular(self):
        step = _step()
        layout = place(SCREEN, None, bubble_size(step))
        callout = Callout(step, layout, on_button=lambda _: None)
        try:
            callout.show()
            pump_gtk(1.0)
            img = grab(DISPLAY)
            # Внутри прямоугольника окна, но вне фигуры — должен просвечивать фон.
            self.assertEqual(img.pixel(layout.bubble.x - 10, layout.bubble.y - 10), BLUE)
            # Внутри самого пузыря — должна быть его белая заливка.
            self.assertEqual(img.pixel(layout.bubble.x + 20, layout.bubble.y + 20), WHITE)
        finally:
            callout.hide()


if __name__ == '__main__':
    unittest.main()
