# Сценарий онбординга: шесть шагов данными, а не кодом. Чистый модуль —
# без gi/GTK, разбор TOML стандартной библиотекой (tomllib, Python 3.11+),
# чтобы сценарий можно было прогнать и проверить и на машине без дисплея.
import tomllib
from collections import namedtuple

TARGET_KINDS = ('none', 'desktop_icon', 'window', 'tray')
_ADVANCE_ON_KINDS = ('button', 'target')

# questions — только у шага 4 (готовые вопросы агенту); у остальных шагов
# пустой кортеж. buttons и questions — кортежи, а не списки: шаг неизменяем
# после загрузки.
Step = namedtuple('Step', 'number title body target buttons advance_on questions')
Target = namedtuple('Target', 'kind desktop_file name_pattern anchor')

_REQUIRED_STEP_FIELDS = ('number', 'title', 'body', 'target', 'buttons', 'advance_on')
_REQUIRED_TARGET_FIELDS = {
    'desktop_icon': ('desktop_file',),
    'window': ('name_pattern',),
}


def load(path: str) -> list[Step]:
    """Читает сценарий из TOML-файла, проверяет и возвращает список шагов.

    Неизвестный kind цели и пропущенное обязательное поле поднимают
    ValueError с номером шага.
    """
    with open(path, 'rb') as f:
        data = tomllib.load(f)
    return [_load_step(raw) for raw in data.get('step', [])]


def _load_step(raw: dict) -> Step:
    number = raw.get('number', '?')
    for field in _REQUIRED_STEP_FIELDS:
        if field not in raw:
            raise ValueError(f'шаг {number}: нет обязательного поля {field!r}')
    if raw['advance_on'] not in _ADVANCE_ON_KINDS:
        raise ValueError(f'шаг {number}: неизвестный advance_on {raw["advance_on"]!r}')
    target = _load_target(raw['target'], number)
    questions = tuple(raw.get('questions', ()))
    return Step(raw['number'], raw['title'], raw['body'], target,
                tuple(raw['buttons']), raw['advance_on'], questions)


def _load_target(raw: dict, step_number) -> Target:
    kind = raw.get('kind')
    if kind not in TARGET_KINDS:
        raise ValueError(f'шаг {step_number}: неизвестный тип цели {kind!r}')
    for field in _REQUIRED_TARGET_FIELDS.get(kind, ()):
        if field not in raw:
            raise ValueError(f'шаг {step_number}: нет обязательного поля цели {field!r}')
    return Target(kind, raw.get('desktop_file'), raw.get('name_pattern'),
                  raw.get('anchor', 'whole'))
