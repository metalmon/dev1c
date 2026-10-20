# Сквозной тест: настоящий scenario.toml через настоящую логику World
# (значки, окна, covered/anchor — не упрощённый FakeWorld) на реалистичном
# списке окон (стол, панель, 1С, Zed сразу, как на живом стенде между
# шагами).
#
# Проверяет только существование: что visible_target() не None на каждом
# шаге, у которого в сценарии есть цель. Это ловит «стрелки нет вовсе» —
# ровно то, из-за чего не рисовалась ни одна стрелка на шагах 4/5 (anchor
# перекрывал сам себя) и было бы поймано для 2/3/6 (стол/панель
# перекрывают всё). Это НЕ ловит «стрелка не там»: если тело не
# переносится по словам или трей считается от рабочей области, а не от
# монитора (тень трея на 43px выше значка), прямоугольник всё равно не
# None — тест пройдёт молча. Под голым Xvfb без панели workarea и monitor
# вообще совпадают, так что даже попытка отличить верную позицию трея от
# неверной здесь ничего бы не поймала — то, что эти два бага не ловятся
# именно здесь, не случайность теста, а предел того, что «цель
# существует» умеет проверить.
import pathlib
import tempfile
import unittest

import scenario
from steps import Machine
from targets import Rect, WindowInfo
from world import World

_HERE = pathlib.Path(__file__).parent
SCENARIO = str(_HERE.parent / 'scenario.toml')

# Снимок окон, который реально мог бы отдать xprop между шагами 2 и 6 —
# нарочно все сразу (а не только то, что нужно шагу под проверкой):
# тест ловит и «стол/панель мешают», и «лишнее чужое окно не мешает».
# x=100 оставляет в стороне столбец значков слева (col=0: центр x=60,
# ширина 48 — область значка кончается на x=84): окно Zed реалистично
# большое, но не залезает на тот самый значок, на который показывает
# шаг 3 — иначе это не «стол/панель больше не мешают», а «большое окно
# закрывает значок», другое, законное поведение covered() (см.
# tests/test_world.py, test_real_window_on_top_of_an_icon_still_covers_it).
_DESKTOP = WindowInfo('Рабочий стол', 'xfdesktop', Rect(0, 0, 1280, 800), stacking=0)
_ONEC = WindowInfo('Управление нашей фирмой (учебная), редакция 3.0', '',
                    Rect(150, 100, 800, 600), stacking=1)
_ZED = WindowInfo('Zed - project', 'dev.zed.Zed', Rect(100, 100, 820, 640), stacking=2)
_PANEL = WindowInfo('xfce4-panel', 'xfce4-panel', Rect(0, 757, 1280, 43), stacking=3)
_REALISTIC_WINDOWS = [_DESKTOP, _ONEC, _ZED, _PANEL]

# Шаги, у которых в сценарии действительно есть на что показывать
# (kind != 'none') — только шаг 1 ни на что не целится.
_STEPS_WITH_A_TARGET = (2, 3, 4, 5, 6)


class _FakeDesktopLayout:
    """Подменяет world._LAYOUT_PATH на временный файл с раскладкой обоих
    значков сценария (1c-start, dev.zed.Zed) и создаёт сами .desktop-
    файлы — desktop_icon() требует, чтобы они существовали (устаревшие
    записи раскладки игнорируются)."""

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self._tmp.name)
        desktop = root / 'Desktop'
        desktop.mkdir()
        (desktop / '1c-start.desktop').touch()
        (desktop / 'dev.zed.Zed.desktop').touch()
        self.layout_path = root / 'icons.screen0.yaml'
        self.layout_path.write_text(
            f'"{desktop / "1c-start.desktop"}":\n  row: 0\n  col: 0\n'
            f'"{desktop / "dev.zed.Zed.desktop"}":\n  row: 1\n  col: 0\n',
            encoding='utf-8')

    def __enter__(self):
        import world
        self._original = world._LAYOUT_PATH
        world._LAYOUT_PATH = self.layout_path
        return self

    def __exit__(self, *exc):
        import world
        world._LAYOUT_PATH = self._original
        self._tmp.cleanup()


class ArrowOnEveryRealTargetTests(unittest.TestCase):
    def test_every_step_with_a_target_gets_an_arrow(self):
        steps = scenario.load(SCENARIO)
        self.assertEqual(steps[0].target.kind, 'none')  # шаг 1 — не в выборке ниже

        with _FakeDesktopLayout():
            world = World(window_lister=lambda: _REALISTIC_WINDOWS)
            missing = []
            for number in _STEPS_WITH_A_TARGET:
                step = steps[number - 1]
                m = Machine([step], world)
                if m.visible_target() is None:
                    missing.append(number)
            self.assertEqual(missing, [],
                              f'шаги без стрелки при реалистичном списке окон: {missing}')

    def test_step_one_has_no_target_by_design(self):
        # Не в цикле выше — это утверждение другого рода («так и
        # задумано»), не «нашлась цель».
        steps = scenario.load(SCENARIO)
        with _FakeDesktopLayout():
            world = World(window_lister=lambda: _REALISTIC_WINDOWS)
            m = Machine([steps[0]], world)
            self.assertIsNone(m.visible_target())


if __name__ == '__main__':
    unittest.main()
