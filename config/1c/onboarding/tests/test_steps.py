# Тесты модуля steps. Чистая логика машины шагов — без gi/GTK, world
# подменяется фейком.
import unittest

from scenario import Step, Target
from targets import Rect, WindowInfo, anchor_in, tray_area
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
        # anchor_in по-настоящему, как и у World.window() — иначе этот
        # фейк не замечает ничего из того, что зависит от обрезки якорем
        # (ровно баг финальной волны: anchor='right-bottom' шагов 4/5).
        pattern = pattern.lower()
        matches = [w for w in self.windows if pattern in w.name.lower()]
        if not matches:
            return None
        return anchor_in(max(matches, key=lambda w: w.stacking).rect, anchor)

    def covered(self, rect, owner=None):
        return self.covered_flag

    def screen(self):
        return self._screen

    def monitor(self):
        # Тестам, которым нужна разница монитор/рабочая область (трей и
        # панель), проще собрать свой World — см. tests/test_world.py.
        return self._screen


def _steps():
    # Четыре шага: none/button, desktop_icon/target, window/target,
    # снова none/button — последний нужен, чтобы увидеть переход ЗА шаг 3,
    # а не просто остановку на нём.
    return [
        Step(1, 'Шаг 1', '', Target('none', None, None, 'whole'), ('Начать',), 'button', ()),
        Step(2, 'Шаг 2', '', Target('desktop_icon', '1c-start.desktop', None, 'whole'),
             ('Дальше',), 'target', ()),
        Step(3, 'Шаг 3', '', Target('window', None, '1cv8', 'whole'), ('Дальше',), 'target', ()),
        Step(4, 'Шаг 4', '', Target('none', None, None, 'whole'), ('Готово',), 'button', ()),
    ]


class MachineTests(unittest.TestCase):
    def test_step_advances_when_its_window_appears(self):
        world = FakeWorld()
        m = Machine(_steps(), world)
        m.press('next')                           # шаг 1 -> 2, цель — значок 1С
        self.assertEqual(m.current.number, 2)
        # Окно шага 3 уже стоит в world — ДО того, как мы туда попали.
        # Совпадение идёт по .name (как в настоящем targets.window), а
        # не по .wclass, поэтому имя должно и впрямь содержать '1cv8'.
        world.windows = [WindowInfo('Конфигуратор 1cv8', '1cv8', WINDOW_RECT, 0)]
        m.tick()                                   # значок на шаге 2 так и не появился
        self.assertEqual(m.current.number, 2)
        world.icons = {'1c-start.desktop': ICON_RECT}
        m.tick()
        self.assertEqual(m.current.number, 3)      # значок появился — шаг 3
        m.tick()
        # Цель шага 3 была готова с самого начала — тик сразу уводит
        # дальше, на шаг 4, а не просто держит нас на месте.
        self.assertEqual(m.current.number, 4)

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

    def test_visible_target_after_close_is_none(self):
        m = Machine(_steps(), FakeWorld())
        m.press('close')
        self.assertIsNone(m.visible_target())       # не должно падать

    def test_visible_target_after_walking_off_the_end_is_none(self):
        world = FakeWorld()
        m = Machine(_steps(), world)
        for _ in range(len(_steps())):
            m.press('next')
        self.assertTrue(m.done)
        self.assertIsNone(m.visible_target())       # не должно падать

    def test_tick_does_not_advance_a_button_step(self):
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT})
        m = Machine(_steps(), world)
        m.tick()                                   # шаг 1: advance_on='button'
        self.assertEqual(m.current.number, 1)


class WaitForTests(unittest.TestCase):
    # Баг, который нашла задача 6 на настоящем World: цель стрелки —
    # значок запуска, который стоит на столе уже при входе; без отдельного
    # условия перехода tick() уходил бы вперёд на первом же тике, не дав
    # человеку даже прочитать шаг. wait_for — отдельный Target именно для
    # этого условия, независимый от того, куда показывает стрелка.
    def _step_with_wait_for(self):
        return Step(
            1, 'Шаг', '', Target('desktop_icon', '1c-start.desktop', None, 'whole'),
            ('Дальше',), 'target', (), wait_for=Target('window', None, '1cv8', 'whole'))

    def test_always_present_icon_does_not_advance_the_step(self):
        # Значок есть с самого начала (как в реальности) — но окна 1С ещё нет.
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT})
        m = Machine([self._step_with_wait_for(),
                     Step(2, 'Шаг 2', '', Target('none', None, None, 'whole'), ('Готово',), 'button', ())],
                    world)
        m.tick()
        self.assertEqual(m.current.number, 1)      # окна 1cv8 нет — не ушли

    def test_arrow_still_points_at_the_icon_while_waiting(self):
        # target (стрелка) и wait_for (условие перехода) — независимы:
        # стрелка остаётся на значке весь шаг, даже пока условие не готово.
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT})
        m = Machine([self._step_with_wait_for()], world)
        self.assertEqual(m.visible_target(), ICON_RECT)

    def test_advances_once_the_window_appears(self):
        world = FakeWorld(icons={'1c-start.desktop': ICON_RECT})
        m = Machine([self._step_with_wait_for(),
                     Step(2, 'Шаг 2', '', Target('none', None, None, 'whole'), ('Готово',), 'button', ())],
                    world)
        m.tick()
        self.assertEqual(m.current.number, 1)
        world.windows = [WindowInfo('Конфигуратор 1cv8', '', WINDOW_RECT, 0)]
        m.tick()
        self.assertEqual(m.current.number, 2)       # окно появилось — ушли

    def test_wait_for_defaults_to_target_when_not_set(self):
        # Старое поведение (задачи 4/5, шаги без wait_for): tick() ждёт
        # сам target, как и до разделения этих двух понятий.
        world = FakeWorld()
        m = Machine(_steps(), world)
        m.press('next')                             # шаг 1 -> 2
        m.tick()
        self.assertEqual(m.current.number, 2)       # значка ещё нет — не ушли
        world.icons = {'1c-start.desktop': ICON_RECT}
        m.tick()
        self.assertEqual(m.current.number, 3)        # target == wait_for по умолчанию


class TrayTests(unittest.TestCase):
    def test_tray_target_uses_monitor_not_workarea(self):
        # Трей сидит в панели, которую screen() (рабочая область)
        # исключает — Machine обязана мерить трей от monitor() (весь
        # монитор целиком), иначе стрелка указывает выше значка на
        # высоту панели (финальная волна).
        class _World(FakeWorld):
            def monitor(self):
                return Rect(0, 0, 1280, 800)  # монитор целиком

        world = _World(screen=Rect(0, 0, 1280, 757))  # рабочая область без 43px панели
        step = Step(1, 'Шаг', '', Target('tray', None, None, 'whole'), ('Готово',), 'button', ())
        m = Machine([step], world)
        self.assertEqual(m.visible_target(), tray_area(Rect(0, 0, 1280, 800)))
        self.assertNotEqual(m.visible_target(), tray_area(Rect(0, 0, 1280, 757)))


if __name__ == '__main__':
    unittest.main()
