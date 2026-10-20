# Тесты модуля targets. Чистая логика, без gi/GTK — запускаются и там,
# где дисплея нет.
import pathlib
import unittest

from targets import (Rect, WindowInfo, anchor_in, connector_area, desktop_icon,
                     tray_area, window)

FIXTURE = (pathlib.Path(__file__).parent / 'fixtures' / 'icons.screen0.yaml').read_text(encoding='utf-8')


class DesktopIconTests(unittest.TestCase):
    def test_icon_of_existing_file(self):
        r = desktop_icon(FIXTURE, '1c-start.desktop', exists=lambda p: True)
        self.assertEqual((r.x + r.w // 2, r.y + r.h // 2), (60, 146))

    def test_stale_entry_is_ignored(self):
        r = desktop_icon(FIXTURE, '1c-enterprise.desktop', exists=lambda p: False)
        self.assertIsNone(r)

    def test_unknown_file(self):
        self.assertIsNone(desktop_icon(FIXTURE, 'нет-такого.desktop', exists=lambda p: True))

    def test_root_icon_at_grid_origin(self):
        # /home/dev1, row 0 col 0 — калибровочная точка сетки, (60, 40).
        r = desktop_icon(FIXTURE, 'home/dev1', exists=lambda p: True)
        self.assertEqual((r.x + r.w // 2, r.y + r.h // 2), (60, 40))


class WindowTests(unittest.TestCase):
    def test_picks_topmost_matching_by_substring(self):
        windows = [
            WindowInfo('Zed - foo.bsl', 'Zed', Rect(0, 0, 800, 600), stacking=1),
            WindowInfo('Zed - bar.bsl', 'Zed', Rect(100, 100, 820, 640), stacking=5),
            WindowInfo('Gitea — вкладка', 'Chromium', Rect(0, 0, 100, 100), stacking=9),
        ]
        r = window(lambda: windows, 'zed')
        self.assertEqual(r, Rect(100, 100, 820, 640))

    def test_no_match_returns_none(self):
        self.assertIsNone(window(lambda: [], 'zed'))

    def test_matches_by_window_class_when_the_title_does_not_say_it(self):
        # На стенде заголовок окна Zed принадлежит открытому проекту
        # («proj — SKILL.md»), слова «Zed» в нём нет вовсе, и шаги 4 и 5
        # оставались без стрелки. Класс окна от содержимого не зависит.
        windows = [
            WindowInfo('proj — SKILL.md', 'dev.zed.Zed', Rect(420, 80, 840, 620), stacking=3),
        ]
        self.assertEqual(window(lambda: windows, 'Zed'), Rect(420, 80, 840, 620))


class ConnectorAreaTests(unittest.TestCase):
    """Место, которое займёт окно коннектора, когда его откроют.

    Шаги про коннектор предлагают человеку открыть окно, и выноска
    обязана оставить ему место заранее: иначе окно открывается ровно под
    выноской, и шаг накрывает собой то, о чём говорит.
    """

    def test_sits_in_the_bottom_right_corner_with_a_gap(self):
        r = connector_area(Rect(0, 0, 2560, 1397))
        self.assertEqual(r.x + r.w + 12, 2560)
        self.assertEqual(r.y + r.h + 12, 1397)

    def test_never_leaves_a_small_work_area(self):
        work = Rect(0, 0, 320, 240)
        r = connector_area(work)
        self.assertGreaterEqual(r.x, work.x)
        self.assertGreaterEqual(r.y, work.y)
        self.assertLessEqual(r.w, work.w)
        self.assertLessEqual(r.h, work.h)


class TrayAreaTests(unittest.TestCase):
    def test_sits_at_bottom_right_of_workarea(self):
        r = tray_area(Rect(0, 0, 1920, 1080))
        self.assertEqual((r.x + r.w, r.y + r.h), (1920, 1080))
        self.assertEqual((r.w, r.h), (160, 40))


class AnchorInTests(unittest.TestCase):
    def test_whole_returns_rect_unchanged(self):
        rect = Rect(0, 0, 300, 400)
        self.assertEqual(anchor_in(rect, 'whole'), rect)

    def test_right_third_full_height(self):
        r = anchor_in(Rect(0, 0, 300, 400), 'right')
        self.assertEqual(r, Rect(200, 0, 100, 400))

    def test_left_third_full_height(self):
        # Дерево файлов проекта в Zed: своего окна у него нет, только
        # доля от окна редактора — как у панели агента справа.
        r = anchor_in(Rect(0, 0, 300, 400), 'left')
        self.assertEqual(r, Rect(0, 0, 100, 400))

    def test_middle_third_full_height(self):
        # Дерево файлов проекта в Zed: у редактора на стенде три колонки,
        # и слева список бесед агента, а не файлы.
        r = anchor_in(Rect(0, 0, 300, 400), 'middle')
        self.assertEqual(r, Rect(100, 0, 100, 400))

    def test_right_bottom_third_by_quarter(self):
        r = anchor_in(Rect(0, 0, 300, 400), 'right-bottom')
        self.assertEqual(r, Rect(200, 300, 100, 100))


if __name__ == '__main__':
    unittest.main()
