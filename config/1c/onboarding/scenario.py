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
#
# actions — что подпись кнопки значит для Machine.press ('next'/'close') и,
# у двух кнопок сценария, что запустить (main.py решает только КАК запустить
# — subprocess.Popen, не дожидаясь; ЧТО запустить — это тоже данные сценария,
# не словарь в коде main.py). Крестик/Esc выноски — не отсюда: их 'close'
# приходит из callout.py напрямую, мимо этой карты.
# actions по умолчанию {} — так старые вызовы Step(...) без него (задачи
# 4 и 5, фейковые шаги в тестах машины и выноски) не ломаются.
#
# wait_for — условие перехода для advance_on='target', отдельно от target
# (куда рисовать стрелку). Это разные вещи: цель шага 2 — значок запуска
# 1С, он на столе уже при входе, а ждать шаг обязан не значок (он виден
# всегда), а окно 1С. По умолчанию None — тогда Machine.tick() ждёт сам
# target, как было до этого разделения (шаги 4/5 ждут то самое окно Zed,
# на которое и показывают, им отдельного wait_for не нужно).
Step = namedtuple('Step', 'number title body target buttons advance_on questions actions wait_for',
                   defaults=({}, None))
Target = namedtuple('Target', 'kind desktop_file name_pattern anchor')
Action = namedtuple('Action', 'action launch')

_ACTION_KINDS = ('next', 'close')
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
    wait_for = _load_target(raw['wait_for'], number) if 'wait_for' in raw else None
    questions = tuple(raw.get('questions', ()))
    actions = _load_actions(raw.get('actions', {}), raw['buttons'], number)
    return Step(raw['number'], raw['title'], raw['body'], target,
                tuple(raw['buttons']), raw['advance_on'], questions, actions, wait_for)


def _load_target(raw: dict, step_number) -> Target:
    kind = raw.get('kind')
    if kind not in TARGET_KINDS:
        raise ValueError(f'шаг {step_number}: неизвестный тип цели {kind!r}')
    for field in _REQUIRED_TARGET_FIELDS.get(kind, ()):
        if field not in raw:
            raise ValueError(f'шаг {step_number}: нет обязательного поля цели {field!r}')
    return Target(kind, raw.get('desktop_file'), raw.get('name_pattern'),
                  raw.get('anchor', 'whole'))


def _load_actions(raw: dict, buttons, step_number) -> dict:
    """Карта подпись кнопки -> Action(action, launch) для одного шага.

    Каждая кнопка шага обязана иметь запись: иначе main.py при клике не
    узнает, что передать Machine.press. launch — команда (кортеж строк)
    для subprocess.Popen или None, если кнопка ничего не запускает.
    """
    actions = {}
    for label in buttons:
        entry = raw.get(label)
        if entry is None:
            raise ValueError(f'шаг {step_number}: нет действия для кнопки {label!r}')
        if isinstance(entry, str):
            action, launch = entry, None
        else:
            action = entry.get('action')
            launch = tuple(entry['launch']) if 'launch' in entry else None
        if action not in _ACTION_KINDS:
            raise ValueError(f'шаг {step_number}: неизвестное действие {action!r} для кнопки {label!r}')
        actions[label] = Action(action, launch)
    return actions
