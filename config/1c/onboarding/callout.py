# Окно выноски онбординга: пузырь с текстом и стрелка к цели — одно
# GTK-окно, а не два. Маска формы строится из альфы того же рисунка,
# которым окно красит себя на экране, поэтому shape_combine_region несёт
# и пузырь, и стрелку сразу. Входная область — только прямоугольник
# пузыря: так курсор проходит сквозь стрелку и всё вокруг неё, и это
# никак не мешает работе в 1С или в Zed под окном. Проверено на стенде
# через RDP без композитора: ни блюра, ни полупрозрачности там нет и
# не нужно — фигура вырезается, а не затемняется.
import math

import cairo
import gi

gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk, Gtk

# HEAD/FOOT/LINE_HEIGHT и три числа вопросов — не свои, а
# placement.bubble_size: число строк, под которое был посчитан размер
# пузыря, и то, что здесь рисуется, обязаны совпадать не только по
# значению, но и по источнику (финальная волна: раньше оба места несли
# одни и те же шесть чисел отдельными строками — ровно та развилка,
# из-за которой в фикс-раунде 2 высота пузыря и нарисованный текст
# однажды чуть не разошлись).
from placement import (BUTTON_BOTTOM_GAP, BUTTON_HEIGHT, FOOT, HEAD, LINE_HEIGHT, QUESTION_GAP,
                       QUESTION_ROW_HEIGHT, QUESTIONS_CAPTION_HEIGHT, wrap_body)
from targets import Rect

# Запас вокруг пузыря и стрелки: под тень и чтобы окно гарантированно
# было больше фигуры (иначе курсору было бы некуда «проходить мимо»).
PAD = 20

BORDER_WIDTH = 3
SHADOW_OFFSET = 6
CLOSE_SIZE = 16
CLOSE_MARGIN = 10
BUTTON_GAP = 10
# Поля внутри кнопки по горизонтали и кегль подписи: кнопка должна быть
# заметной целью мыши, а не полоской в один текст.
BUTTON_PAD_X = 18
BUTTON_FONT_SIZE = 14
# Наконечник обязан быть заметно шире хвоста, иначе стрелка читается
# как жирная чёрточка с точкой на конце — ровно так она и выглядела в
# упор на 2560x1440. При прежних 14px длины и развороте в 25 градусов
# наконечник выходил шириной 11.8 при хвосте в 6.
ARROW_WIDTH = 4
ARROW_HEAD = 18
ARROW_SPREAD_DEG = 32
# Остриё не упирается в цель, а останавливается чуть раньше: иначе
# треугольник ложится прямо на значок, о котором и идёт речь.
ARROW_GAP = 6

# Палитра из утверждённых макетов.
_WHITE = (1, 1, 1)
_BORDER = (0x1B / 255, 0x1B / 255, 0x1B / 255)
_COUNTER = (0x8A / 255, 0x6D / 255, 0x00 / 255)
_BUTTON = (0xE6 / 255, 0xB8 / 255, 0x00 / 255)
_SECONDARY_BORDER = (0x88 / 255, 0x88 / 255, 0x88 / 255)
_SHADOW = (0x4D / 255, 0x4D / 255, 0x4D / 255)
# Бледная подложка строки вопроса — макеты рисуют вопросы рамкой на
# бледном фоне, не белым, как сам пузырь (иначе их не отличить от тела).
_QUESTION_BG = (0xF2 / 255, 0xF1 / 255, 0xEC / 255)
# Пометка «скопировано» — мягкий янтарный оттенок той же подложки, не
# просто смена рамки: тонкую границу легко не заметить, а заливку видно
# сразу (и пиксель её можно проверить тестом надёжно, рамку в 1px — нет).
_QUESTION_BG_COPIED = (0xF7 / 255, 0xE8 / 255, 0xB8 / 255)


def _content_rect(layout):
    """Прямоугольник пузыря и концов стрелки вместе — то, что рисуем."""
    b = layout.bubble
    xs = (b.x, b.x + b.w, layout.arrow_start[0], layout.arrow_end[0])
    ys = (b.y, b.y + b.h, layout.arrow_start[1], layout.arrow_end[1])
    x0, y0 = min(xs), min(ys)
    return Rect(x0, y0, max(xs) - x0, max(ys) - y0)


def _window_rect(layout):
    """Прямоугольник окна — контент плюс запас PAD со всех сторон."""
    c = _content_rect(layout)
    return Rect(c.x - PAD, c.y - PAD, c.w + 2 * PAD, c.h + 2 * PAD)


class Callout:
    """Выноска: один POPUP-попап, вырезанный по контуру своего рисунка."""

    def __init__(self, step, layout, on_button, copy_question=None, total=None, scale=1.0, position=None):
        self._step = step
        # Все числа рисования заданы в логических пикселях, посчитанных
        # под экран примерно 1280x800. На 2560x1440 выноска занимала бы
        # вчетверо меньшую долю экрана, а текст в 14px и стрелка в 72px
        # читались бы как царапины — ровно это и увидел заказчик на
        # своём сеансе 20.10.2026. Поэтому вся геометрия считается в
        # логических единицах, а экранные координаты получаются
        # умножением на scale; рисование делает то же самое одним
        # cr.scale, не трогая ни одной константы.
        self._scale = scale
        # Сколько всего шагов — приходит из сценария (Machine.total), а
        # не зашито в рисование: иначе добавленный шаг молча оставляет
        # в подписи старое число. None (тесты, которым счётчик не важен) —
        # подпись без «из M», а не выдуманное число.
        self._total = total
        # Номер шага внутри акта; None — берём сквозной номер самого шага.
        self._position = position
        self._layout = layout
        self._on_button = on_button
        # Готовые вопросы шага 4 копируются в буфер сами, без похода через
        # Machine — клик на вопрос не двигает шаг и не трогает on_button.
        # copy_question — та же функция, что main.py уже тестирует отдельно
        # (tests.test_clipboard); опциональна, чтобы задачи 4/5 и старые
        # тесты, которым вопросы не нужны, не должны были её передавать.
        self._copy_question = copy_question
        self._close_rect = None
        self._button_rects = ()
        self._question_rects = ()
        self._copied_question = None

        window_rect = _window_rect(layout)
        self._origin = (int(window_rect.x * scale), int(window_rect.y * scale))
        self._logical_origin = (window_rect.x, window_rect.y)
        self._width = int(round(window_rect.w * scale))
        self._height = int(round(window_rect.h * scale))

        self.window = Gtk.Window(type=Gtk.WindowType.POPUP)
        self.window.set_app_paintable(True)
        self.window.set_keep_above(True)
        self.window.set_accept_focus(False)
        self.window.set_default_size(self._width, self._height)
        self.window.move(*self._origin)
        self.window.add_events(Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.KEY_PRESS_MASK)
        self.window.connect('draw', self._on_draw)
        self.window.connect('button-press-event', self._on_click)
        self.window.connect('key-press-event', self._on_key)

    def show(self):
        """Показывает выноску и вырезает её форму по контуру рисунка."""
        self.window.show_all()
        self._apply_shape()

    def hide(self):
        self.window.hide()

    def destroy(self):
        """Освобождает само GTK-окно и его поверхности — не прячет, а
        закрывает совсем. main.py зовёт эту, не hide(), когда заменяет
        одну выноску другой: на каждый шаг/каждый тик создаётся новый
        Callout, и без destroy() старые окна копились бы в памяти, пока
        программа не закрыта (финальная волна)."""
        self.window.destroy()

    # -- рисование -----------------------------------------------------

    def _on_draw(self, _widget, cr):
        cr.scale(self._scale, self._scale)
        self._draw(cr)
        return False

    def _apply_shape(self):
        # Та же функция рисования — на отдельной поверхности, без экрана —
        # даёт маску формы по альфе непрозрачных пикселей. Входная область
        # (куда реагирует мышь) — только прямоугольник пузыря, тем же
        # способом через заливку.
        shape_surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, self._width, self._height)
        shape_cr = cairo.Context(shape_surface)
        shape_cr.scale(self._scale, self._scale)
        self._draw(shape_cr)
        self.window.shape_combine_region(Gdk.cairo_region_create_from_surface(shape_surface))

        input_surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, self._width, self._height)
        icr = cairo.Context(input_surface)
        icr.scale(self._scale, self._scale)
        bx, by = self._local(self._layout.bubble.x, self._layout.bubble.y)
        icr.rectangle(bx, by, self._layout.bubble.w, self._layout.bubble.h)
        icr.set_source_rgba(0, 0, 0, 1)
        icr.fill()
        self.window.input_shape_combine_region(Gdk.cairo_region_create_from_surface(input_surface))

    def _local(self, x, y):
        ox, oy = self._logical_origin
        return x - ox, y - oy

    def _draw(self, cr):
        step = self._step
        bubble = self._layout.bubble
        bx, by = self._local(bubble.x, bubble.y)
        bw, bh = bubble.w, bubble.h

        if self._layout.arrow_start != self._layout.arrow_end:
            self._draw_arrow(cr)

        # Тень — сплошная, без блюра, смещена вниз-вправо.
        cr.set_source_rgb(*_SHADOW)
        cr.rectangle(bx + SHADOW_OFFSET, by + SHADOW_OFFSET, bw, bh)
        cr.fill()

        cr.set_source_rgb(*_WHITE)
        cr.rectangle(bx, by, bw, bh)
        cr.fill()
        cr.set_source_rgb(*_BORDER)
        cr.set_line_width(BORDER_WIDTH)
        cr.rectangle(bx + BORDER_WIDTH / 2, by + BORDER_WIDTH / 2,
                     bw - BORDER_WIDTH, bh - BORDER_WIDTH)
        cr.stroke()

        # Один и тот же список строк для счёта высоты (placement.bubble_size)
        # и для рисования — никакого второго разбиения здесь: разойдись они,
        # высота пузыря и нарисованный текст снова разъедутся (фикс-раунд 2).
        body_lines = wrap_body(step.body)

        self._draw_counter(cr, bx, by, step)
        self._draw_title(cr, bx, by, step)
        self._draw_body(cr, bx, by, body_lines)
        self._question_rects = self._draw_questions(cr, bx, by, bw, step, body_lines)
        self._close_rect = self._draw_close(cr, bx, by, bw)
        self._button_rects = self._draw_buttons(cr, bx, by, bw, bh, step.buttons)

    def _draw_arrow(self, cr):
        sx, sy = self._local(*self._layout.arrow_start)
        ex, ey = self._local(*self._layout.arrow_end)
        angle = math.atan2(ey - sy, ex - sx)
        # Отступаем от цели и укорачиваем хвост до основания наконечника:
        # круглый колпачок иначе вылезал бы за остриё прямо на цель.
        ex -= ARROW_GAP * math.cos(angle)
        ey -= ARROW_GAP * math.sin(angle)
        base_x = ex - ARROW_HEAD * math.cos(angle) * 0.9
        base_y = ey - ARROW_HEAD * math.sin(angle) * 0.9

        cr.set_source_rgb(*_BORDER)
        cr.set_line_width(ARROW_WIDTH)
        cr.set_line_cap(cairo.LINE_CAP_BUTT)
        cr.move_to(sx, sy)
        cr.line_to(base_x, base_y)
        cr.stroke()

        spread = math.radians(ARROW_SPREAD_DEG)
        x1 = ex - ARROW_HEAD * math.cos(angle - spread)
        y1 = ey - ARROW_HEAD * math.sin(angle - spread)
        x2 = ex - ARROW_HEAD * math.cos(angle + spread)
        y2 = ey - ARROW_HEAD * math.sin(angle + spread)
        cr.move_to(ex, ey)
        cr.line_to(x1, y1)
        cr.line_to(x2, y2)
        cr.close_path()
        cr.fill()

    def _draw_counter(self, cr, x, y, step):
        cr.set_source_rgb(*_COUNTER)
        cr.select_font_face('sans-serif', cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
        cr.set_font_size(13)
        cr.move_to(x + 16, y + 20)
        number = step.number if self._position is None else self._position
        counter = f'Шаг {number}' if self._total is None else f'Шаг {number} из {self._total}'
        cr.show_text(counter)

    def _draw_title(self, cr, x, y, step):
        cr.set_source_rgb(*_BORDER)
        cr.select_font_face('sans-serif', cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
        cr.set_font_size(16)
        cr.move_to(x + 16, y + 44)
        cr.show_text(step.title)

    def _draw_body(self, cr, x, y, body_lines):
        # body_lines — то, что вернул placement.wrap_body (посчитано один
        # раз в _draw); здесь только рисуем, ничего заново не разбиваем.
        cr.set_source_rgb(*_BORDER)
        cr.select_font_face('sans-serif', cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_NORMAL)
        cr.set_font_size(14)
        for i, line in enumerate(body_lines):
            cr.move_to(x + 16, y + HEAD + (i + 1) * LINE_HEIGHT - 6)
            cr.show_text(line)

    def _draw_questions(self, cr, x, y, w, step, body_lines):
        """Готовые вопросы шага 4 — строкой-коробкой каждый, на бледной
        подложке, под телом и над кнопками (bubble_size уже выделил им
        место — см. placement.py). Подпись-пояснение рисуется только
        тогда, когда вопросы вообще есть: это не текст сценария, а общая
        подсказка механики клика, одна и та же для любого шага."""
        if not step.questions:
            return ()

        row_y = y + HEAD + len(body_lines) * LINE_HEIGHT
        row_w = w - 32

        cr.set_source_rgb(*_SECONDARY_BORDER)
        cr.select_font_face('sans-serif', cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_NORMAL)
        cr.set_font_size(12)
        cr.move_to(x + 16, row_y + QUESTIONS_CAPTION_HEIGHT - 6)
        cr.show_text('Нажмите на вопрос — он скопируется в буфер обмена')
        row_y += QUESTIONS_CAPTION_HEIGHT

        rects = []
        for text in step.questions:
            copied = text == self._copied_question

            cr.set_source_rgb(*(_QUESTION_BG_COPIED if copied else _QUESTION_BG))
            cr.rectangle(x + 16, row_y, row_w, QUESTION_ROW_HEIGHT)
            cr.fill()
            cr.set_source_rgb(*(_BUTTON if copied else _SECONDARY_BORDER))
            cr.set_line_width(2 if copied else 1)
            cr.rectangle(x + 16, row_y, row_w, QUESTION_ROW_HEIGHT)
            cr.stroke()

            cr.set_source_rgb(*_BORDER)
            cr.select_font_face('sans-serif', cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_NORMAL)
            cr.set_font_size(13)
            cr.move_to(x + 24, row_y + QUESTION_ROW_HEIGHT // 2 + 5)
            cr.show_text(text)
            if copied:
                # Отметка, что клик сработал — рядом с текстом, тем же
                # цветом, что счётчик шага, а не ещё одним новым оттенком.
                cr.set_source_rgb(*_COUNTER)
                cr.show_text('   ✓ скопировано')

            rects.append((text, Rect(int(x + 16), int(row_y), int(row_w), QUESTION_ROW_HEIGHT)))
            row_y += QUESTION_ROW_HEIGHT + QUESTION_GAP
        return rects

    def _draw_close(self, cr, x, y, w):
        r = Rect(x + w - CLOSE_MARGIN - CLOSE_SIZE, y + CLOSE_MARGIN, CLOSE_SIZE, CLOSE_SIZE)
        cr.set_source_rgb(*_BORDER)
        cr.set_line_width(2)
        cr.move_to(r.x, r.y)
        cr.line_to(r.x + r.w, r.y + r.h)
        cr.move_to(r.x + r.w, r.y)
        cr.line_to(r.x, r.y + r.h)
        cr.stroke()
        return r

    def _draw_buttons(self, cr, x, y, w, h, buttons):
        rects = []
        by = y + h - BUTTON_HEIGHT - BUTTON_BOTTOM_GAP
        bx = x + w - 16
        cr.select_font_face('sans-serif', cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_NORMAL)
        cr.set_font_size(BUTTON_FONT_SIZE)
        for label in reversed(buttons):
            # Первая кнопка списка — главное действие шага (так решено в
            # утверждённых макетах на каждом экране: «Начать» важнее
            # «Закрыть», «Открыть 1С» важнее «Дальше» и т.д.) — она и
            # закрашивается; остальные — вторичные, без заливки.
            primary = label == buttons[0]
            extents = cr.text_extents(label)
            btn_w = extents.width + 2 * BUTTON_PAD_X
            bx -= btn_w

            if primary:
                cr.set_source_rgb(*_BUTTON)
                cr.rectangle(bx, by, btn_w, BUTTON_HEIGHT)
                cr.fill()

            cr.set_source_rgb(*(_BORDER if primary else _SECONDARY_BORDER))
            cr.set_line_width(2 if primary else 1)
            cr.rectangle(bx, by, btn_w, BUTTON_HEIGHT)
            cr.stroke()

            cr.set_source_rgb(*_BORDER)
            # Базовая линия по центру кнопки, а не на глаз от нижнего
            # края: высота кнопки теперь задаётся константой, и подпись
            # обязана ездить вместе с ней.
            cr.move_to(bx + BUTTON_PAD_X, by + (BUTTON_HEIGHT - extents.height) / 2 - extents.y_bearing)
            cr.show_text(label)

            rects.append((label, Rect(int(bx), int(by), int(btn_w), BUTTON_HEIGHT)))
            bx -= BUTTON_GAP
        return rects

    # -- ввод ------------------------------------------------------------

    def _on_click(self, _widget, event):
        # Прямоугольники кнопок посчитаны в логических единицах, а
        # событие приходит в экранных — делим, иначе на масштабе больше
        # единицы попадать в кнопки пришлось бы мимо них.
        ex, ey = event.x / self._scale, event.y / self._scale
        if self._hit(self._close_rect, ex, ey):
            self._on_button('close')
            return True
        for text, rect in self._question_rects:
            if self._hit(rect, ex, ey):
                self._copy_clicked_question(text)
                return True
        for label, rect in self._button_rects:
            if self._hit(rect, ex, ey):
                self._on_button(label)
                return True
        return False

    def _copy_clicked_question(self, text):
        # Не идёт через on_button/Machine: копирование в буфер не двигает
        # шаг и не закрывает выноску — только помечает строку и просит
        # перерисоваться (без пересборки формы окна: размер фигуры не
        # меняется, меняется только то, что внутри неё нарисовано).
        if self._copy_question is not None:
            self._copy_question(text)
        self._copied_question = text
        self.window.queue_draw()

    def _on_key(self, _widget, event):
        # Окно не принимает фокус, поэтому до этого обработчика клавиша
        # может просто не дойти — крестик в пузыре остаётся единственным
        # закрытием, на которое можно рассчитывать.
        if event.keyval == Gdk.KEY_Escape:
            self._on_button('close')
            return True
        return False

    @staticmethod
    def _hit(rect, x, y):
        return rect is not None and rect.x <= x <= rect.x + rect.w and rect.y <= y <= rect.y + rect.h
