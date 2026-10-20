# Сценарий онбординга: шесть шагов данными, а не кодом. Чистый модуль —
# без gi/GTK, разбор TOML стандартной библиотекой (tomllib, Python 3.11+),
# чтобы сценарий можно было прогнать и проверить и на машине без дисплея.
import tomllib
from collections import namedtuple
from types import MappingProxyType

from targets import ANCHOR_KINDS

TARGET_KINDS = ('none', 'desktop_icon', 'window', 'tray', 'connector')
_ADVANCE_ON_KINDS = ('button', 'target')

# questions — только у шага 4 (готовые вопросы агенту); у остальных шагов
# пустой кортеж. buttons и questions — кортежи, а не списки: шаг неизменяем
# после загрузки.
#
# actions — что подпись кнопки значит для Machine.press ('next'/'close',
# либо 'stay' — выполнить launch/close_window и остаться на месте, не
# трогая Machine вовсе: кнопка шага 3 закрывает 1С и открывает редактор,
# но шаг уходит дальше по wait_for — окну Zed, — а не по самому клику)
# и, у части кнопок сценария, что запустить и какое окно попросить
# закрыться (main.py решает только КАК — subprocess.Popen, не дожидаясь,
# и вежливый запрос закрытия, не kill; ЧТО и КАКОЕ — это данные сценария,
# не словарь в коде main.py). Крестик/Esc выноски — не отсюда: их 'close'
# приходит из callout.py напрямую, мимо этой карты.
# actions по умолчанию — пустая MappingProxyType, не {}: пустой словарь
# как default намеренно один объект на все Step(...) без actions (задачи
# 4 и 5, фейковые шаги в тестах машины и выноски) — MappingProxyType той
# же пустоты нельзя случайно испортить присваиванием в элемент, {} можно.
#
# wait_for — условие перехода для advance_on='target', отдельно от target
# (куда рисовать стрелку). Это разные вещи: цель шага 2 — значок запуска
# 1С, он на столе уже при входе, а ждать шаг обязан не значок (он виден
# всегда), а окно 1С. По умолчанию None — тогда Machine.tick() ждёт сам
# target, как было до этого разделения (шаги 4/5 ждут то самое окно Zed,
# на которое и показывают, им отдельного wait_for не нужно).
# act — к какому акту принадлежит шаг. Первый акт показывает, что агент
# умеет читать базу, второй — как правка агента в неё возвращается.
# Счётчик «Шаг N из M» считает внутри акта, а не по всему сценарию:
# первый экран обещает «семь коротких шагов», и «Шаг 1 из 14» сделало бы
# это обещание ложью, хотя второй акт ещё и запускается по желанию.
# По умолчанию 1 — шаги первого акта поля не несут.
# reserve — что оставить свободным на экране, даже если стрелка
# показывает в другую сторону. Сейчас одно значение, 'connector': окно
# коннектора встаёт в правый нижний угол, и шаг, который предлагает его
# открыть, обязан это место освободить заранее.
Step = namedtuple('Step',
                   'number title body target buttons advance_on questions actions wait_for act reserve',
                   defaults=(MappingProxyType({}), None, 1, None))
Target = namedtuple('Target', 'kind desktop_file name_pattern anchor')
Action = namedtuple('Action', 'action launch close_window', defaults=(None,))

_ACTION_KINDS = ('next', 'close', 'stay')
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
                tuple(raw['buttons']), raw['advance_on'], questions, actions, wait_for,
                int(raw.get('act', 1)), _load_reserve(raw.get('reserve'), number))


RESERVE_KINDS = ('connector',)


def _load_reserve(value, step_number):
    if value is None:
        return None
    if value not in RESERVE_KINDS:
        raise ValueError(f'шаг {step_number}: неизвестный reserve {value!r}')
    return value


def _load_target(raw: dict, step_number) -> Target:
    kind = raw.get('kind')
    if kind not in TARGET_KINDS:
        raise ValueError(f'шаг {step_number}: неизвестный тип цели {kind!r}')
    for field in _REQUIRED_TARGET_FIELDS.get(kind, ()):
        if field not in raw:
            raise ValueError(f'шаг {step_number}: нет обязательного поля цели {field!r}')
    anchor = raw.get('anchor', 'whole')
    # Опечатка в anchor обязана упасть здесь, на загрузке, а не когда-то
    # потом внутри targets.anchor_in на живом стенде, в середине тика
    # (финальная волна).
    if anchor not in ANCHOR_KINDS:
        raise ValueError(f'шаг {step_number}: неизвестный anchor {anchor!r}')
    return Target(kind, raw.get('desktop_file'), raw.get('name_pattern'), anchor)


def _load_actions(raw: dict, buttons, step_number) -> MappingProxyType:
    """Карта подпись кнопки -> Action(action, launch, close_window) для
    одного шага, неизменяемая (MappingProxyType — та же причина, что у
    default в Step выше).

    Каждая кнопка шага обязана иметь запись: иначе main.py при клике не
    узнает, что передать Machine.press. launch — команда (кортеж строк)
    для subprocess.Popen или None, если кнопка ничего не запускает.
    close_window — название окна (image_pattern для World.close_window),
    которое кнопка должна вежливо попросить закрыться, или None.
    """
    actions = {}
    for label in buttons:
        entry = raw.get(label)
        if entry is None:
            raise ValueError(f'шаг {step_number}: нет действия для кнопки {label!r}')
        if isinstance(entry, str):
            action, launch, close_window = entry, None, None
        else:
            action = entry.get('action')
            launch = tuple(entry['launch']) if 'launch' in entry else None
            close_window = entry.get('close_window')
        if action not in _ACTION_KINDS:
            raise ValueError(f'шаг {step_number}: неизвестное действие {action!r} для кнопки {label!r}')
        actions[label] = Action(action, launch, close_window)
    return MappingProxyType(actions)
