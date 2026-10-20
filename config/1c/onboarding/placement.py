# Размещение выноски онбординга: чистая геометрия — без gi/GTK, чтобы
# задача 4 рисовала по тем же числам, что здесь посчитаны и проверены без
# дисплея. bubble_size считается тут, а не в окне: иначе размер выноски
# зависел бы от обмера текста в GTK, а размещение — от размера, и круг
# бы не разорвался. По той же причине перенос строк тела (wrap_body) —
# тоже здесь: высота, которую считает bubble_size, и строки, которые
# рисует callout, обязаны быть одним и тем же списком, иначе высота
# меньше нарисованного и фигура снова обрезает текст (фикс-раунд 2:
# ровно так и было — тело шага 1 было одной длинной строкой без переноса,
# рисовалось за правый край пузыря и обрезалось маской формы).
import textwrap
from collections import namedtuple

from targets import Rect

Layout = namedtuple('Layout', 'bubble arrow_start arrow_end')

# Калибровка: сколько символов тела укладывается в одну строку пузыря
# при ширине по умолчанию (640px, поля 16px с каждой стороны — те же
# числа, что PAD/x+16 в callout.py — итого 608px на текст). Шрифт тела
# фиксирован (callout._draw_body: 'sans-serif', NORMAL, 14px), значит
# число детерминировано — не угадано, а измерено в контейнере cairo-
# слепком (2026-10-20): семь настоящих абзацев сценария (1116 символов)
# на этом шрифте и размере дали от 7.81 до 8.08 px/символ, в среднем
# 7.97 px/символ — то есть 608 / 7.97 ≈ 76 символов в строку при средней
# ширине букв. Взято с запасом вниз, а не по верхней границе измерения:
# длинные слова и заглавные («Ж», «Щ», «М») локально шире среднего, и
# короче строка безопаснее, чем ещё одна обрезанная маской.
BODY_CHARS_PER_LINE = 70


def wrap_body(body: str, chars_per_line: int = BODY_CHARS_PER_LINE) -> list[str]:
    """Переносит тело шага по словам в список строк не шире
    chars_per_line символов, сохраняя пустые строки — разрывы абзацев,
    на которые опираются тексты сценария (scenario.toml). callout.py
    рисует ровно то, что вернула эта функция, без своего разбиения —
    два места, считающие строки по-разному, расходятся рано или поздно.
    """
    lines = []
    for paragraph in body.splitlines():
        if not paragraph.strip():
            lines.append('')
        else:
            lines.extend(textwrap.wrap(paragraph, width=chars_per_line))
    return lines


def bubble_size(step, width: int = 640, line_height: int = 21, head: int = 64, foot: int = 24,
                 question_row: int = 32, question_gap: int = 8, questions_caption: int = 20,
                 chars_per_line: int = BODY_CHARS_PER_LINE) -> tuple[int, int]:
    """Размер выноски по числу перенесённых строк тела шага (wrap_body)
    плюс, если они есть у шага, готовые вопросы — каждый своей строкой-
    кнопкой, не частью текста тела (фикс-раунд 1, задача 6: без этого
    места под вопросы callout не считал, и строки рисовались за
    пределами фигуры)."""
    lines = wrap_body(step.body, chars_per_line)
    height = head + foot + len(lines) * line_height
    if step.questions:
        height += questions_caption + len(step.questions) * (question_row + question_gap)
    return (width, height)


def _overlaps(a: Rect, b: Rect) -> bool:
    return not (a.x + a.w <= b.x or b.x + b.w <= a.x or a.y + a.h <= b.y or b.y + b.h <= a.y)


def _fits_screen(r: Rect, screen: Rect) -> bool:
    return (screen.x <= r.x and screen.y <= r.y
            and r.x + r.w <= screen.x + screen.w
            and r.y + r.h <= screen.y + screen.h)


def _candidates(target: Rect, bw: int, bh: int, margin: int):
    # Порядок предпочтений: справа, слева, снизу, сверху.
    cx = target.x + target.w // 2 - bw // 2
    cy = target.y + target.h // 2 - bh // 2
    yield Rect(target.x + target.w + margin, cy, bw, bh)
    yield Rect(target.x - margin - bw, cy, bw, bh)
    yield Rect(cx, target.y + target.h + margin, bw, bh)
    yield Rect(cx, target.y - margin - bh, bw, bh)


def _fallback(screen: Rect, target: Rect, bw: int, bh: int) -> Rect:
    # Ни один вариант не влез целиком и мимо цели — прижимаемся к краю,
    # противоположному цели, но не выходим за экран: читаемость выноски
    # важнее, чем свобода от цели.
    screen_cx = screen.x + screen.w // 2
    screen_cy = screen.y + screen.h // 2
    target_cx = target.x + target.w // 2
    target_cy = target.y + target.h // 2
    x = screen.x if target_cx > screen_cx else screen.x + screen.w - bw
    y = screen.y if target_cy > screen_cy else screen.y + screen.h - bh
    return Rect(x, y, bw, bh)


def _arrow(bubble: Rect, target: Rect):
    bcx, bcy = bubble.x + bubble.w // 2, bubble.y + bubble.h // 2
    tcx, tcy = target.x + target.w // 2, target.y + target.h // 2
    dx, dy = tcx - bcx, tcy - bcy
    if abs(dx) >= abs(dy):
        start = (bubble.x + bubble.w if dx > 0 else bubble.x, bcy)
    else:
        start = (bcx, bubble.y + bubble.h if dy > 0 else bubble.y)
    return start, (tcx, tcy)


def place(screen: Rect, target: Rect | None, bubble_size: tuple[int, int], margin: int = 24) -> Layout:
    """Прямоугольник выноски и концы стрелки для данного экрана и цели."""
    bw, bh = bubble_size
    if target is None:
        x = screen.x + screen.w // 2 - bw // 2
        y = screen.y + screen.h // 2 - bh // 2
        bubble = Rect(x, y, bw, bh)
        centre = (bubble.x + bubble.w // 2, bubble.y + bubble.h // 2)
        return Layout(bubble, centre, centre)

    for candidate in _candidates(target, bw, bh, margin):
        if _fits_screen(candidate, screen) and not _overlaps(candidate, target):
            bubble = candidate
            break
    else:
        bubble = _fallback(screen, target, bw, bh)

    start, end = _arrow(bubble, target)
    return Layout(bubble, start, end)
