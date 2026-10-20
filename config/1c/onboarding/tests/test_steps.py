# Тесты модуля steps. Чистая логика машины шагов — без gi/GTK, world
# подменяется фейком.
import unittest

from scenario import Step, Target
from targets import Rect, WindowInfo
from steps import Machine

SCREEN = Rect(0, 0, 1280, 800)
ICON_RECT = Rect(36, 122, 48, 48)
WINDOW_RECT = Rect(100, 100, 820, 640)


class FakeWorld:
    """Фейковый мир: значки и окна — по спискам, трей — не нужен тестам."""

    def __init__(self, icons=None, windows=None, covered=False, screen=SCREEN):
        self.icons = icons or {}
        self.windows = windows or []
        self.covered_flag = covered
        self._screen = screen

    def icon(self, desktop_file):
        return self.icons.get(desktop_file)

    def window(self, pattern, anchor='whole'):
        pattern = pattern.lower()
        matches = [w for w in self.windows if pattern in w.name.lower()]
        if not matches:
            return None
        return max(matches, key=lambda w: w.stacking).rect

    def covered(self, rect):
        return self.covered_flag

    def screen(self):
        return self._screen


def _steps():
    # Три шага: none/button, desktop_icon/target, window/target — этого
    # хватает, чтобы проверить все переходы и видимость цели.
    return [
        Step(1, 'Шаг 1', '', Target('none', None, None, 'whole'), ('Начать',), 'button', ()),
        Step(2, 'Шаг 2', '', Target('desktop_icon', '1c-start.desktop', None, 'whole'),
             ('Дальше',), 'target', ()),
        Step(3, 'Шаг 3', '', Target('window', None, '1cv8', 'whole'), ('Дальше',), 'target', ()),
    ]


class MachineTests(unittest.TestCase):
    def test_step_advances_when_its_window_appears(self):
        world = FakeWorld()
        m = Machine(_steps(), world)
        m.press('next')                           # шаг 1 -> 2, цель — значок 1С
        self.assertEqual(m.current.number, 2)
        world.windows = [WindowInfo('Управление нашей фирмой (учебная)', '1cv8', WINDOW_RECT, 0)]
        m.tick()                                   # значок на шаге 2 так и не появился
        self.assertEqual(m.current.number, 2)
        world.icons = {'1c-start.desktop': ICON_RECT}
        m.tick()
        self.assertEqual(m.current.number, 3)      # значок появился — шаг 3
        m.tick()
        self.assertEqual(m.current.number, 3)      # окно уже стояло в world — тоже готово к переходу

    def test_tick_does_nothing_until_target_appears(self):
        world = FakeWorld()
        m = Machine(_steps(), world)
        m.press('next')
        m.tick()
        self.assertEqual(m.current.number, 2)

    def test_arrow_hidden_while_target_is_covered(self):
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT}, covered=True)
        m = Machine(_steps(), world)
        m.press('next')
        self.assertIsNone(m.visible_target())

    def test_arrow_hidden_when_target_kind_is_none(self):
        world = FakeWorld()
        m = Machine(_steps(), world)
        self.assertIsNone(m.visible_target())      # шаг 1: kind='none'

    def test_arrow_hidden_when_target_not_on_screen(self):
        world = FakeWorld()                        # значка нигде нет
        m = Machine(_steps(), world)
        m.press('next')
        self.assertIsNone(m.visible_target())

    def test_arrow_shown_when_target_visible_and_uncovered(self):
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT})
        m = Machine(_steps(), world)
        m.press('next')
        self.assertEqual(m.visible_target(), ICON_RECT)

    def test_next_always_works_even_without_the_target(self):
        world = FakeWorld()
        m = Machine(_steps(), world)
        for _ in range(len(_steps())):
            m.press('next')
        self.assertTrue(m.done)

    def test_close_ends_everything(self):
        m = Machine(_steps(), FakeWorld())
        m.press('close')
        self.assertTrue(m.done)

    def test_close_from_a_later_step(self):
        m = Machine(_steps(), FakeWorld())
        m.press('next')
        m.press('close')
        self.assertTrue(m.done)

    def test_current_is_none_once_done(self):
        m = Machine(_steps(), FakeWorld())
        m.press('close')
        self.assertIsNone(m.current)

    def test_tick_after_done_is_safe(self):
        m = Machine(_steps(), FakeWorld())
        m.press('close')
        m.tick()                                   # не должно падать
        self.assertTrue(m.done)

    def test_tick_does_not_advance_a_button_step(self):
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT})
        m = Machine(_steps(), world)
        m.tick()                                   # шаг 1: advance_on='button'
        self.assertEqual(m.current.number, 1)


if __name__ == '__main__':
    unittest.main()
