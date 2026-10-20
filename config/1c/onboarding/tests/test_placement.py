# Тесты модуля placement. Чистая геометрия — без gi/GTK.
import math
import unittest

from scenario import Step, Target
from targets import Rect, tray_area
from placement import FOOT, HEAD, bubble_size, place, wrap_body

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

    def test_fallback_when_sliding_genuinely_cannot_help(self):
        # Цель занимает почти весь экран — ни с одной стороны нет места
        # даже в 24px (margin), и сдвиг вдоль края (финальная правка)
        # тут не спасает ни одного из четырёх кандидатов: это и есть
        # настоящий, не выдуманный fallback-случай, не просто «центр
        # кандидата попал на край», как было у трея в углу раньше (тот
        # случай сдвиг теперь решает без fallback — см. TrayPlacementTests).
        screen = Rect(0, 0, 800, 600)
        target = Rect(50, 50, 700, 500)  # 50px до каждого края — меньше, чем нужно
        layout = place(screen, target, (640, 150))
        self.assertTrue(on_boundary(layout.bubble, layout.arrow_start))
        self.assertTrue(on_boundary(target, layout.arrow_end))

    def test_arrow_end_is_on_the_edge_not_the_centre(self):
        # Анти-цель спецификации: наконечник не должен закрывать то, на
        # что показывает. Раньше конец стрелки был ровно центром цели.
        target = Rect(36, 122, 48, 48)
        layout = place(SCREEN, target, (640, 150))
        centre = (target.x + target.w // 2, target.y + target.h // 2)
        self.assertNotEqual(layout.arrow_end, centre)
        self.assertTrue(on_boundary(target, layout.arrow_end))


class TrayPlacementTests(unittest.TestCase):
    # Баг координатора на шаге 6: трей — узкая (160px) цель у самого
    # нижнего правого угла. Единственный кандидат, который к ней в
    # принципе подходит, — «сверху» (справа/слева не хватает высоты у
    # самого низа экрана, снизу — только пустота за краем монитора), а
    # «сверху» центрирован по x на цель и вылезает за правый край.
    # Переполнение раньше отбрасывало кандидата целиком, роняя всё в
    # _fallback — противоположный (левый верхний) угол, стрелку через
    # весь экран. Экран здесь согласован сам с собой (трей посчитан от
    # того же прямоугольника, что передан как экран) — реальный
    # перекос world.screen()/world.monitor() в main.py (экран — рабочая
    # область без панели, а трей меряется от монитора целиком) не
    # предмет этой правки.
    SCREEN = Rect(0, 0, 1280, 800)
    MARGIN = 24
    BUBBLE_SIZE = (640, 235)  # типичный размер шага 6 — два абзаца текста

    def _tray_layout(self):
        target = tray_area(self.SCREEN)
        return place(self.SCREEN, target, self.BUBBLE_SIZE, margin=self.MARGIN), target

    def test_bubble_sits_above_the_tray_inside_the_screen_with_margin(self):
        layout, target = self._tray_layout()
        b = layout.bubble
        # Выше цели (трея), не сбоку и не ниже — туда, где его рисует
        # утверждённый макет.
        self.assertLessEqual(b.y + b.h, target.y)
        # Внутри экрана, с отступом, а не впритык к рамке (координатор:
        # «give it the margin the rest of the placement uses»).
        self.assertGreaterEqual(b.x, self.SCREEN.x + self.MARGIN)
        self.assertGreaterEqual(b.y, self.SCREEN.y + self.MARGIN)
        self.assertLessEqual(b.x + b.w, self.SCREEN.x + self.SCREEN.w - self.MARGIN)
        self.assertLessEqual(b.y + b.h, self.SCREEN.y + self.SCREEN.h - self.MARGIN)

    def test_tray_arrow_stays_short(self):
        # Ровно то свойство, которое было не так, и единственное, что
        # видит человек: раньше стрелка шла через весь экран по
        # диагонали (~1200px). Длина относительно диагонали экрана, а
        # не точка — число говорит «коротко», не «именно сюда».
        layout, _ = self._tray_layout()
        sx, sy = layout.arrow_start
        ex, ey = layout.arrow_end
        length = math.hypot(ex - sx, ey - sy)
        diagonal = math.hypot(self.SCREEN.w, self.SCREEN.h)
        self.assertLess(length, diagonal * 0.2)


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
        self.assertEqual(h, HEAD + FOOT)

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

    def test_long_body_counts_wrapped_lines_not_logical_lines(self):
        # Фикс-раунд 2: тело сценария — один длинный абзац без переносов
        # (TOML "\"-продолжение превращает его в одну логическую строку).
        # bubble_size обязан выделить место под перенесённые строки, а не
        # под одну — иначе высота меньше того, что реально нарисовано, и
        # фигура обрежет текст (ровно баг со скриншота координатора).
        long_body = ('Здесь уже стоит учебная 1С с «Управлением нашей фирмой» и редактор, '
                     'в котором живёт ИИ-агент. Ставить ничего не нужно, ломать тут нечего. '
                     'Шесть коротких шагов, минуты три.')
        step = Step(1, 'Заголовок', long_body, _TARGET, ('Дальше',), 'button', ())
        logical_lines = 1
        self.assertGreater(len(long_body.splitlines()), 0)
        self.assertEqual(len(long_body.splitlines()), logical_lines)
        _, h = bubble_size(step)
        wrapped = len(wrap_body(long_body))
        self.assertGreater(wrapped, logical_lines)
        self.assertEqual(h, HEAD + FOOT + wrapped * 21)


class WrapBodyTests(unittest.TestCase):
    def test_long_paragraph_wraps_to_several_lines_that_all_fit(self):
        body = ('Здесь уже стоит учебная 1С с «Управлением нашей фирмой» и редактор, '
                'в котором живёт ИИ-агент. Ставить ничего не нужно, ломать тут нечего. '
                'Шесть коротких шагов, минуты три.')
        lines = wrap_body(body, chars_per_line=40)
        self.assertGreater(len(lines), 1)
        for line in lines:
            self.assertLessEqual(len(line), 40)
        # Слова не потеряны и не переставлены — просто расставлены по строкам.
        self.assertEqual(' '.join(lines), body)

    def test_blank_line_between_paragraphs_survives_wrapping(self):
        body = 'Первый абзац, короткий.\n\nВторой абзац, тоже короткий.'
        lines = wrap_body(body, chars_per_line=40)
        self.assertIn('', lines)
        blank_at = lines.index('')
        before = ' '.join(lines[:blank_at])
        after = ' '.join(lines[blank_at + 1:])
        self.assertEqual(before, 'Первый абзац, короткий.')
        self.assertEqual(after, 'Второй абзац, тоже короткий.')

    def test_short_body_is_a_single_line(self):
        self.assertEqual(wrap_body('Коротко.', chars_per_line=70), ['Коротко.'])

    def test_empty_body_wraps_to_no_lines(self):
        self.assertEqual(wrap_body('', chars_per_line=70), [])


class ArrowShapeTests(unittest.TestCase):
    """Две поломки, найденные глазами на стенде 20.10.2026.

    Прежние тесты стрелки проверяли только направление и то, что остриё
    стоит на грани цели — и обе эти поломки пропустили.
    """

    SCREEN = Rect(0, 0, 1280, 800)

    def _length(self, layout):
        (x0, y0), (x1, y1) = layout.arrow_start, layout.arrow_end
        return math.hypot(x1 - x0, y1 - y0)

    def _inside(self, rect, point, slack=1):
        x, y = point
        return (rect.x + slack < x < rect.x + rect.w - slack
                and rect.y + slack < y < rect.y + rect.h - slack)

    def test_arrow_does_not_run_back_under_the_bubble(self):
        # Шаг 6: трей в правом нижнем углу, выноска встаёт над ним и
        # шире его. Стрелка начиналась на ПРАВОЙ грани выноски и шла
        # обратно влево под ней — на экране оставался оторванный хвост.
        tray = Rect(1120, 760, 160, 40)
        layout = place(self.SCREEN, tray, (640, 275))
        for t in range(0, 101):
            point = (layout.arrow_start[0] + (layout.arrow_end[0] - layout.arrow_start[0]) * t / 100,
                     layout.arrow_start[1] + (layout.arrow_end[1] - layout.arrow_start[1]) * t / 100)
            self.assertFalse(self._inside(layout.bubble, point),
                             'стрелка проходит сквозь выноску в точке %s' % (point,))

    def test_arrow_is_long_enough_to_read_as_an_arrow(self):
        # Значок на столе слева: стрелка была 24px при наконечнике в 14,
        # то есть чёрточка, приклеенная к значку.
        icon = Rect(36, 334, 48, 48)
        layout = place(self.SCREEN, icon, (640, 191))
        self.assertGreaterEqual(self._length(layout), 48)

    def test_arrow_starts_on_the_bubble_edge_and_ends_on_the_target_edge(self):
        icon = Rect(36, 334, 48, 48)
        layout = place(self.SCREEN, icon, (640, 191))
        self.assertFalse(self._inside(layout.bubble, layout.arrow_start))
        self.assertFalse(self._inside(icon, layout.arrow_end))
        # Остриё — на грани цели, а не в стороне от неё.
        self.assertTrue(icon.x <= layout.arrow_end[0] <= icon.x + icon.w)
        self.assertTrue(icon.y <= layout.arrow_end[1] <= icon.y + icon.h)



if __name__ == '__main__':
    unittest.main()
