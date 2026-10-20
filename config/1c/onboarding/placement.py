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

# Единственный источник этих шести чисел (финальная волна): раньше
# HEAD/FOOT/LINE_HEIGHT и три числа вопросов были названы константами и
# тут, и ещё раз в callout.py — тем же значением, но отдельной строкой,
# которую легко поправить в одном месте и забыть про другое. Именно
# такое расхождение дало клип текста в фикс-раунде 2 (туда он так и не
# дошёл — повезло, что числа совпадали; в следующий раз могло не
# повезти). Теперь callout.py импортирует их отсюда, не переобъявляет.
HEAD = 64
# Подвал — это ровно полоса кнопок с воздухом сверху и снизу. Держим его
# производным от трёх чисел, а не отдельной константой: раньше FOOT=24 в
# точности равнялся кнопке в 20px плюс 4px отступа, и любое увеличение
# кнопки молча съедало зазор над ней.
BUTTON_HEIGHT = 30
BUTTON_TOP_GAP = 14
BUTTON_BOTTOM_GAP = 20
FOOT = BUTTON_TOP_GAP + BUTTON_HEIGHT + BUTTON_BOTTOM_GAP
LINE_HEIGHT = 21
QUESTIONS_CAPTION_HEIGHT = 20
QUESTION_ROW_HEIGHT = 32
QUESTION_GAP = 8

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


def bubble_size(step, width: int = 640, line_height: int = LINE_HEIGHT, head: int = HEAD, foot: int = FOOT,
                 question_row: int = QUESTION_ROW_HEIGHT, question_gap: int = QUESTION_GAP,
                 questions_caption: int = QUESTIONS_CAPTION_HEIGHT,
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


def _candidates(target: Rect, bw: int, bh: int, margin: int, screen: Rect):
    # Порядок предпочтений: справа, слева, снизу, сверху.
    #
    # «Снизу»/«сверху» центрированы по x на цель — для цели заметно уже
    # пузыря (например, 160px трей против 640px пузыря) это уводит
    # кандидата за боковой край экрана. Раньше такой кандидат просто
    # выбрасывался (overflow -> reject), роняя всё в _fallback и
    # противоположный угол — ровно баг с шестым шагом: стрелка через
    # весь экран, пузырь в углу без отступов. Кандидат на верной
    # стороне от цели, который всего лишь вылез за край, не выбрасывают
    # — сдвигают вдоль этого края внутрь экрана, с тем же отступом
    # margin, что и у экрана вообще (сам экран не трогаем, трогаем
    # только x этих двух кандидатов).
    #
    # «Справа»/«слева» не сдвигаются по y: если по вертикали места не
    # хватило, сторона, скорее всего, в принципе не годится (цель
    # слишком близко к верхнему/нижнему краю) — следующий кандидат или
    # честный _fallback лучше, чем пузырь, уехавший от цели далеко по
    # вертикали только ради формального «не пересекается».
    cx = target.x + target.w // 2 - bw // 2
    cy = target.y + target.h // 2 - bh // 2
    yield Rect(target.x + target.w + margin, cy, bw, bh)
    yield Rect(target.x - margin - bw, cy, bw, bh)
    below_above_x = min(max(cx, screen.x + margin), screen.x + screen.w - bw - margin)
    yield Rect(below_above_x, target.y + target.h + margin, bw, bh)
    yield Rect(below_above_x, target.y - margin - bh, bw, bh)


def _fallback(screen: Rect, target: Rect, bw: int, bh: int, margin: int) -> Rect:
    # Ни один вариант не влез целиком и мимо цели — прижимаемся к краю,
    # противоположному цели, но не выходим за экран: читаемость выноски
    # важнее, чем свобода от цели. С отступом margin, тем же, что и у
    # остального размещения — иначе пузырь садится впритык к рамке
    # экрана, без зазора вообще.
    screen_cx = screen.x + screen.w // 2
    screen_cy = screen.y + screen.h // 2
    target_cx = target.x + target.w // 2
    target_cy = target.y + target.h // 2
    x = screen.x + margin if target_cx > screen_cx else screen.x + screen.w - bw - margin
    y = screen.y + margin if target_cy > screen_cy else screen.y + screen.h - bh - margin
    return Rect(x, y, bw, bh)


def _arrow(bubble: Rect, target: Rect):
    # Конец стрелки — на ближней грани цели, не в её центре: наконечник
    # не должен закрывать то, на что показывает (финальная волна; явная
    # анти-цель спецификации). Та же логика, что уже была у начала
    # стрелки (грань пузыря, а не его центр) — зеркально, для цели.
    bcx, bcy = bubble.x + bubble.w // 2, bubble.y + bubble.h // 2
    tcx, tcy = target.x + target.w // 2, target.y + target.h // 2
    dx, dy = tcx - bcx, tcy - bcy
    if abs(dx) >= abs(dy):
        start = (bubble.x + bubble.w if dx > 0 else bubble.x, bcy)
        end = (target.x if dx > 0 else target.x + target.w, tcy)
    else:
        start = (bcx, bubble.y + bubble.h if dy > 0 else bubble.y)
        end = (tcx, target.y if dy > 0 else target.y + target.h)
    return start, end


def place(screen: Rect, target: Rect | None, bubble_size: tuple[int, int], margin: int = 24) -> Layout:
    """Прямоугольник выноски и концы стрелки для данного экрана и цели."""
    bw, bh = bubble_size
    if target is None:
        x = screen.x + screen.w // 2 - bw // 2
        y = screen.y + screen.h // 2 - bh // 2
        bubble = Rect(x, y, bw, bh)
        centre = (bubble.x + bubble.w // 2, bubble.y + bubble.h // 2)
        return Layout(bubble, centre, centre)

    for candidate in _candidates(target, bw, bh, margin, screen):
        if _fits_screen(candidate, screen) and not _overlaps(candidate, target):
            bubble = candidate
            break
    else:
        bubble = _fallback(screen, target, bw, bh, margin)

    start, end = _arrow(bubble, target)
    return Layout(bubble, start, end)
