# Размещение выноски онбординга: чистая геометрия — без gi/GTK, чтобы
# задача 4 рисовала по тем же числам, что здесь посчитаны и проверены без
# дисплея. bubble_size считается тут, а не в окне: иначе размер выноски
# зависел бы от обмера текста в GTK, а размещение — от размера, и круг
# бы не разорвался.
from collections import namedtuple

from targets import Rect

Layout = namedtuple('Layout', 'bubble arrow_start arrow_end')


def bubble_size(step, width: int = 640, line_height: int = 21, head: int = 64, foot: int = 24) -> tuple[int, int]:
    """Размер выноски по числу строк тела шага. Ширина — фиксированное число."""
    lines = step.body.splitlines()
    height = head + foot + len(lines) * line_height
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
