# Тесты модуля placement. Чистая геометрия — без gi/GTK.
import unittest

from scenario import Step, Target
from targets import Rect
from placement import bubble_size, place

SCREEN = Rect(0, 0, 2560, 1440)

_TARGET = Target('none', None, None, 'whole')


def overlaps(a, b):
    """Пересекаются ли два прямоугольника (для assertFalse в тестах)."""
    return not (a.x + a.w <= b.x or b.x + b.w <= a.x or a.y + a.h <= b.y or b.y + b.h <= a.y)


def on_boundary(rect, point):
    """Лежит ли точка на границе прямоугольника (а не где-то внутри/снаружи)."""
    x, y = point
    on_vertical_edge = x in (rect.x, rect.x + rect.w) and rect.y <= y <= rect.y + rect.h
    on_horizontal_edge = y in (rect.y, rect.y + rect.h) and rect.x <= x <= rect.x + rect.w
    return on_vertical_edge or on_horizontal_edge


def inside(rect, point):
    """Лежит ли точка внутри (включая границу) прямоугольника."""
    x, y = point
    return rect.x <= x <= rect.x + rect.w and rect.y <= y <= rect.y + rect.h


class PlaceTests(unittest.TestCase):
    def test_bubble_never_leaves_the_screen(self):
        for target in (Rect(36, 122, 48, 48), Rect(2500, 1400, 24, 24), Rect(1280, 0, 48, 48)):
            b = place(SCREEN, target, (640, 150)).bubble
            self.assertGreaterEqual(b.x, 0)
            self.assertGreaterEqual(b.y, 0)
            self.assertLessEqual(b.x + b.w, SCREEN.w)
            self.assertLessEqual(b.y + b.h, SCREEN.h)

    def test_bubble_never_covers_its_target(self):
        target = Rect(36, 122, 48, 48)
        b = place(SCREEN, target, (640, 150)).bubble
        self.assertFalse(overlaps(b, target))

    def test_small_screen_still_fits(self):
        b = place(Rect(0, 0, 1280, 800), Rect(36, 330, 48, 48), (640, 150)).bubble
        self.assertLessEqual(b.x + b.w, 1280)

    def test_no_target_centres_the_bubble(self):
        layout = place(SCREEN, None, (640, 150))
        self.assertEqual(layout.arrow_start, layout.arrow_end)

    def test_screen_with_non_zero_origin(self):
        # Экран может не начинаться в (0, 0) — проверяем, что это не зашито.
        screen = Rect(1920, 0, 2560, 1440)
        target = Rect(1956, 122, 48, 48)
        b = place(screen, target, (640, 150)).bubble
        self.assertGreaterEqual(b.x, screen.x)
        self.assertGreaterEqual(b.y, screen.y)
        self.assertLessEqual(b.x + b.w, screen.x + screen.w)
        self.assertLessEqual(b.y + b.h, screen.y + screen.h)


class ArrowTests(unittest.TestCase):
    # Проверяем контракт стрелки (где начало и где конец), а не то, по
    # какой оси выбрана грань — это внутренняя эвристика, которую вольна
    # поменять задача 4.
    def test_far_left_target_arrow_reaches_the_target(self):
        target = Rect(36, 122, 48, 48)
        layout = place(SCREEN, target, (640, 150))
        self.assertTrue(on_boundary(layout.bubble, layout.arrow_start))
        self.assertTrue(inside(target, layout.arrow_end))

    def test_bottom_right_tray_target_is_a_fallback_case(self):
        # Эта цель не влезает ни в одно из четырёх предпочтительных мест —
        # место 2 в комментарии «куда поставили» подтвердило: это fallback.
        target = Rect(2500, 1400, 24, 24)
        layout = place(SCREEN, target, (640, 150))
        self.assertTrue(on_boundary(layout.bubble, layout.arrow_start))
        self.assertEqual(layout.arrow_end, (target.x + target.w // 2, target.y + target.h // 2))


class BubbleSizeTests(unittest.TestCase):
    def _step(self, body):
        return Step(1, 'Заголовок', body, _TARGET, ('Дальше',), 'button', ())

    def test_width_is_fixed_height_grows_with_lines(self):
        one_line = bubble_size(self._step('Одна строка.'))
        three_lines = bubble_size(self._step('Первая.\nВторая.\nТретья.'))
        self.assertEqual(one_line[0], 640)
        self.assertEqual(three_lines[0], 640)
        self.assertEqual(three_lines[1] - one_line[1], 2 * 21)

    def test_head_and_foot_included(self):
        w, h = bubble_size(self._step('Одна строка.'), head=64, foot=24, line_height=21)
        self.assertEqual(h, 64 + 24 + 1 * 21)

    def test_empty_body_is_still_valid(self):
        w, h = bubble_size(self._step(''))
        self.assertEqual(h, 64 + 24)

    def test_custom_width(self):
        w, h = bubble_size(self._step('Строка.'), width=480)
        self.assertEqual(w, 480)

    def test_questions_grow_the_bubble(self):
        # Шаг 4: три готовых вопроса — каждый своей строкой внутри пузыря,
        # не просто текстом тела. Без этого bubble_size возвращает место
        # только под body, а callout рисует вопросы за пределами фигуры.
        no_questions = Step(1, 'Заголовок', 'Текст.', _TARGET, ('Дальше',), 'button', ())
        with_questions = Step(1, 'Заголовок', 'Текст.', _TARGET, ('Дальше',), 'button',
                               ('Вопрос один', 'Вопрос два', 'Вопрос три'))
        h_plain = bubble_size(no_questions)[1]
        h_with_questions = bubble_size(with_questions)[1]
        self.assertGreater(h_with_questions, h_plain)

    def test_each_question_adds_the_same_height(self):
        one = Step(1, 'Заголовок', 'Текст.', _TARGET, ('Дальше',), 'button', ('Вопрос один',))
        two = Step(1, 'Заголовок', 'Текст.', _TARGET, ('Дальше',), 'button', ('Вопрос один', 'Вопрос два'))
        h_one = bubble_size(one)[1]
        h_two = bubble_size(two)[1]
        three = Step(1, 'Заголовок', 'Текст.', _TARGET, ('Дальше',), 'button',
                     ('Вопрос один', 'Вопрос два', 'Вопрос три'))
        h_three = bubble_size(three)[1]
        self.assertEqual(h_two - h_one, h_three - h_two)


if __name__ == '__main__':
    unittest.main()
