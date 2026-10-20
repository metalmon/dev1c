# Тесты модуля world. Настоящий World, не фейк: _list_windows подменяется
# через конструктор (World(window_lister=...)) готовым списком WindowInfo
# того же вида, что отдаёт живой xprop/xwininfo — covered()/window()
# проверяются той же логикой исключения стола и панели, что работает на
# стенде, а не отдельным дублирующим её фейком. Требует gi (World его
# импортирует на уровне модуля) — запускается под Xvfb, как
# test_callout_shape.py.
import unittest

from scenario import Step, Target
from steps import Machine
from targets import Rect, WindowInfo
from world import World

SCREEN = Rect(0, 0, 1280, 800)
ICON_RECT = Rect(36, 122, 48, 48)

# Ровно то, что реально отдаёт живой стенд (см. world.py,
# _NOT_COVERING_WCLASSES): стол во весь экран и нижняя панель — то, что
# нашёл координатор на настоящей RDP-сессии и что воспроизведено здесь
# под Xvfb+xfwm4+xfdesktop+xfce4-panel при разработке фикса.
DESKTOP = WindowInfo('Рабочий стол', 'xfdesktop', Rect(0, 0, 1280, 800), stacking=0)
PANEL = WindowInfo('xfce4-panel', 'xfce4-panel', Rect(0, 757, 1280, 43), stacking=1)


def _icon_step():
    return Step(1, 'Шаг', '', Target('desktop_icon', '1c-start.desktop', None, 'whole'),
                ('Дальше',), 'target', ())


def _window_step(name_pattern='Zed'):
    return Step(4, 'Шаг', '', Target('window', None, name_pattern, 'whole'),
                ('Дальше',), 'button', ())


class CoveredByDesktopAndPanelTests(unittest.TestCase):
    # Баг координатора: xfdesktop — окно во весь экран, и без исключения
    # covered() считало его «чем-то поверх» вообще любой цели (у значка
    # нет своего stacking, чтобы отличить «подо мной» от «надо мной»).
    # Стрелка не рисовалась ни на одном шаге ни на одном живом стенде.
    def test_desktop_and_panel_do_not_cover_an_icon_target(self):
        world = World(window_lister=lambda: [DESKTOP, PANEL])
        world.icon = lambda desktop_file: ICON_RECT  # не трогаем файловую систему в тесте
        m = Machine([_icon_step()], world)
        self.assertEqual(m.visible_target(), ICON_RECT)

    def test_real_window_on_top_of_an_icon_still_covers_it(self):
        # Поведение, которое нужно сохранить: с живого эксперимента — editor
        # развёрнут на весь экран, стрелка на значке под ним правильно
        # не рисуется.
        editor = WindowInfo('Zed - project', 'dev.zed.Zed', Rect(0, 0, 1280, 800), stacking=1)
        world = World(window_lister=lambda: [DESKTOP, PANEL, editor])
        world.icon = lambda desktop_file: ICON_RECT
        m = Machine([_icon_step()], world)
        self.assertIsNone(m.visible_target())

    def test_desktop_and_panel_do_not_cover_a_window_target(self):
        # Та же проверка для window-целей шагов 4 и 5 (Zed): окно само
        # стоит в списке со своим stacking, но стол и панель всё равно не
        # должны считаться перекрывающими его.
        zed = WindowInfo('Zed - project', 'dev.zed.Zed', Rect(100, 100, 820, 640), stacking=2)
        world = World(window_lister=lambda: [DESKTOP, zed, PANEL])
        m = Machine([_window_step('Zed')], world)
        self.assertEqual(m.visible_target(), zed.rect)

    def test_real_window_on_top_of_a_window_target_still_covers_it(self):
        zed = WindowInfo('Zed - project', 'dev.zed.Zed', Rect(100, 100, 820, 640), stacking=1)
        dialog = WindowInfo('Подтверждение', '', Rect(300, 300, 200, 100), stacking=2)
        world = World(window_lister=lambda: [DESKTOP, zed, dialog, PANEL])
        m = Machine([_window_step('Zed')], world)
        self.assertIsNone(m.visible_target())


if __name__ == '__main__':
    unittest.main()
